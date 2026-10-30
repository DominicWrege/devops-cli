use crate::{
    auth,
    cli::*,
    config::Target,
    error,
    output::{self, rows, scalar, text},
};
use anyhow::{Context, Result, ensure};
use azure_core::http::{
    Context as HttpContext, RawResponse, Request, RetryOptions,
    policies::{Policy, PolicyResult},
};
use azure_devops_rust_api::{self as ado, git::models as git};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use std::sync::Arc;

// SDK request builders own URLs, authentication, transport and API versions.
// Reading their raw response also preserves fields omitted by generated models.
macro_rules! call {
    ($request:expr) => {{
        let response = send($request.send()).await?.into_raw_response();
        decode(response)
    }};
}

async fn send<T>(future: impl std::future::Future<Output = azure_core::Result<T>>) -> Result<T> {
    tokio::time::timeout(std::time::Duration::from_secs(60), future).await
        .map_err(|_| anyhow::anyhow!("Azure DevOps request timed out. A write may have reached the server; inspect the resource before repeating it."))?
        .map_err(error::sdk)
}

fn decode(response: RawResponse) -> Result<Value> {
    let body = response.into_body();
    if body.as_ref().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body.as_ref())
        .map_err(|_| anyhow::anyhow!("Azure DevOps returned an invalid response"))
}

pub fn segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

mod review;
pub fn branch(value: &str) -> String {
    if value.starts_with("refs/") {
        value.into()
    } else {
        format!("refs/heads/{value}")
    }
}

// The SDK omits YAML path/repository in CreatePipelineConfigurationParameters.
// This per-call policy repairs that payload while retaining the SDK operation.
// It also allows a sparse build PATCH instead of resending read-only Build fields.
struct SdkBody(Value);
impl std::fmt::Debug for SdkBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SdkBody { .. }")
    }
}
#[async_trait::async_trait]
impl Policy for SdkBody {
    async fn send(
        &self,
        ctx: &HttpContext,
        request: &mut Request,
        next: &[Arc<dyn Policy>],
    ) -> PolicyResult {
        request.set_body(azure_core::json::to_json(&self.0)?);
        next_policy(ctx, request, next).await
    }
}

#[derive(Debug)]
struct SdkPath;
#[async_trait::async_trait]
impl Policy for SdkPath {
    async fn send(
        &self,
        ctx: &HttpContext,
        request: &mut Request,
        next: &[Arc<dyn Policy>],
    ) -> PolicyResult {
        // Url displays a root endpoint with a trailing slash; the generated SDK
        // adds another slash when joining the organization/API path.
        let path = format!("/{}", request.url().path().trim_start_matches('/'));
        request.url_mut().set_path(&path);
        // API clients must receive authentication errors, not browser sign-in redirects.
        request.insert_header("x-tfs-fedauthredirect", "Suppress");
        next_policy(ctx, request, next).await
    }
}

// The generated SDK has no connectionData builder. Reuse its authenticated GET
// pipeline and raw response handling with a route override scoped to this call.
#[derive(Debug)]
struct SdkConnectionData;
#[async_trait::async_trait]
impl Policy for SdkConnectionData {
    async fn send(
        &self,
        ctx: &HttpContext,
        request: &mut Request,
        next: &[Arc<dyn Policy>],
    ) -> PolicyResult {
        let path = request
            .url()
            .path()
            .trim_end_matches("/_apis/projects")
            .to_owned();
        request
            .url_mut()
            .set_path(&format!("{path}/_apis/connectionData"));
        request
            .url_mut()
            .query_pairs_mut()
            .clear()
            .append_pair("api-version", "7.1-preview.1");
        next_policy(ctx, request, next).await
    }
}
async fn next_policy(
    ctx: &HttpContext<'_>,
    request: &mut Request,
    next: &[Arc<dyn Policy>],
) -> PolicyResult {
    let (policy, remaining) = next.split_first().ok_or_else(|| {
        azure_core::Error::with_message(
            azure_core::error::ErrorKind::Other,
            "SDK policy chain has no transport",
        )
    })?;
    policy.send(ctx, request, remaining).await
}
fn policies(body: Option<Value>) -> Vec<Arc<dyn Policy>> {
    let mut result: Vec<Arc<dyn Policy>> = vec![Arc::new(SdkPath)];
    if let Some(body) = body {
        result.push(Arc::new(SdkBody(body)));
    }
    result
}

#[derive(Clone)]
pub struct Service {
    credential: ado::Credential,
    endpoint: azure_core::http::Url,
    profile_endpoint: azure_core::http::Url,
    pub target: Target,
}

impl Service {
    pub fn new(credential: Arc<auth::Credential>, target: Target) -> Result<Self> {
        Ok(Self {
            credential: credential.sdk(),
            endpoint: "https://dev.azure.com".parse()?,
            profile_endpoint: "https://app.vssps.visualstudio.com".parse()?,
            target,
        })
    }
    fn git(&self) -> ado::git::Client {
        self.git_body(None)
    }
    fn git_body(&self, body: Option<Value>) -> ado::git::Client {
        ado::git::ClientBuilder::new(self.credential.clone())
            .endpoint(self.endpoint.clone())
            .retry(RetryOptions::none())
            .per_call_policies(policies(body))
            .build()
    }
    fn pipelines(&self, body: Option<Value>) -> ado::pipelines::Client {
        ado::pipelines::ClientBuilder::new(self.credential.clone())
            .endpoint(self.endpoint.clone())
            .retry(RetryOptions::none())
            .per_call_policies(policies(body))
            .build()
    }
    fn build(&self, body: Option<Value>) -> ado::build::Client {
        ado::build::ClientBuilder::new(self.credential.clone())
            .endpoint(self.endpoint.clone())
            .retry(RetryOptions::none())
            .per_call_policies(policies(body))
            .build()
    }
    fn org(&self) -> &str {
        &self.target.organization
    }
    fn project(&self) -> String {
        segment(&self.target.project)
    }

    /// Organization-scoped identity works with PATs; the global Profiles API requires Entra.
    pub async fn current_user_id(&self) -> Result<String> {
        let mut policies = policies(None);
        policies.push(Arc::new(SdkConnectionData));
        let value = call!(
            ado::core::ClientBuilder::new(self.credential.clone())
                .endpoint(self.endpoint.clone())
                .retry(RetryOptions::none())
                .per_call_policies(policies)
                .build()
                .projects_client()
                .list(self.org())
        )?;
        let id = uuid::Uuid::parse_str(text(&value["authenticatedUser"], "id"))
            .context("Azure DevOps returned no valid authenticated user ID")?;
        ensure!(!id.is_nil(), "Azure DevOps returned an anonymous user");
        Ok(id.to_string())
    }
    pub async fn profile(&self) -> Result<Value> {
        call!(
            ado::profile::ClientBuilder::new(self.credential.clone())
                .endpoint(self.profile_endpoint.clone())
                .retry(RetryOptions::none())
                .per_call_policies(policies(None))
                .build()
                .profiles_client()
                .get("me")
        )
    }
    pub async fn organizations(&self) -> Result<Vec<Value>> {
        let profile = self.profile().await?;
        let id = text(&profile, "id");
        ensure!(!id.is_empty(), "Azure DevOps profile has no ID");
        Ok(rows(&call!(
            ado::accounts::ClientBuilder::new(self.credential.clone())
                .endpoint(self.profile_endpoint.clone())
                .retry(RetryOptions::none())
                .per_call_policies(policies(None))
                .build()
                .accounts_client()
                .list()
                .member_id(id)
        )?))
    }
    pub async fn projects(&self) -> Result<Vec<Value>> {
        let client = ado::core::ClientBuilder::new(self.credential.clone())
            .endpoint(self.endpoint.clone())
            .retry(RetryOptions::none())
            .per_call_policies(policies(None))
            .build();
        let mut all = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = client.projects_client().list(self.org()).top(100);
            if let Some(token) = token.as_ref() {
                request = request.continuation_token(
                    token
                        .parse()
                        .context("Invalid project continuation token")?,
                );
            }
            let response = send(request.send()).await?.into_raw_response();
            let next = continuation(&response);
            all.extend(rows(&decode(response)?));
            if next.is_none() || next == token {
                break;
            }
            token = next;
        }
        Ok(all)
    }
    pub async fn check_access(&self) -> Result<()> {
        let client = ado::core::ClientBuilder::new(self.credential.clone())
            .endpoint(self.endpoint.clone())
            .retry(RetryOptions::none())
            .per_call_policies(policies(None))
            .build();
        let projects = call!(client.projects_client().list(self.org()).top(1))?;
        ensure!(
            projects["value"].is_array(),
            "Cannot verify Azure DevOps project-listing access"
        );
        Ok(())
    }
    pub async fn repo_list(&self) -> Result<Vec<Value>> {
        Ok(rows(&call!(
            self.git()
                .repositories_client()
                .list(self.org(), self.project())
        )?))
    }
    pub async fn pipeline_edit(
        &self,
        id: i32,
        name: Option<&str>,
        folder: Option<&str>,
    ) -> Result<Value> {
        if let Some(name) = name {
            PipelineCommand::Rename {
                id,
                name: name.into(),
            }
            .validate()?;
        }
        if let Some(folder) = folder {
            PipelineCommand::Move {
                id,
                folder: folder.into(),
            }
            .validate()?;
        }
        self.require_yaml(id).await?;
        let mut definition = call!(self.build(None).definitions_client().get(
            self.org(),
            self.project(),
            id
        ))?;
        if let Some(name) = name {
            definition["name"] = json!(name);
        }
        if let Some(folder) = folder {
            definition["path"] = json!(format!(
                "\\{}",
                folder.replace('/', "\\").trim_matches('\\')
            ));
        }
        let body: ado::build::models::BuildDefinition = serde_json::from_value(definition.clone())?;
        call!(self.build(Some(definition)).definitions_client().update(
            self.org(),
            body,
            self.project(),
            id
        ))
    }
    pub async fn repo_create(&self, name: &str) -> Result<Value> {
        repository(name)?;
        call!(self.git().repositories_client().create(
            self.org(),
            git::GitRepositoryCreateOptions {
                name: Some(name.into()),
                ..Default::default()
            },
            self.project()
        ))
    }
    pub async fn pr_list(
        &self,
        state: PrState,
        repo: Option<&str>,
        reviewer: Option<&str>,
        author: Option<&str>,
    ) -> Result<Vec<Value>> {
        let mut result = Vec::new();
        let mut skip = 0;
        loop {
            let mut request = self
                .git()
                .pull_requests_client()
                .get_pull_requests_by_project(self.org(), self.project())
                .search_criteria_status(serde_json::from_value::<git::PullRequestStatus>(json!(
                    state.api()
                ))?)
                .top(100)
                .skip(skip);
            if let Some(repo) = repo {
                request = request.search_criteria_repository_id(repo);
            }
            if let Some(reviewer) = reviewer {
                request = request.search_criteria_reviewer_id(reviewer);
            }
            if let Some(author) = author {
                request = request.search_criteria_creator_id(author);
            }
            let page = rows(&call!(request)?);
            let count = page.len();
            result.extend(page);
            if count < 100 {
                break;
            }
            skip += 100;
        }
        Ok(result)
    }
    pub async fn pr(&self, id: i32) -> Result<Value> {
        positive(id)?;
        call!(self.git().pull_requests_client().get_pull_request_by_id(
            self.org(),
            id,
            self.project()
        ))
    }
    fn repo_id(pr: &Value) -> Result<String> {
        pr["repository"]["id"]
            .as_str()
            .map(segment)
            .context("PR response has no repository ID")
    }
    pub async fn attention_prs(&self) -> Result<Vec<Value>> {
        let me = self.current_user_id().await?;
        Ok(self
            .pr_list(PrState::Active, None, Some(&me), None)
            .await?
            .into_iter()
            .filter(|pr| {
                pr["isDraft"] != true
                    && rows(&pr["reviewers"]).iter().any(|r| {
                        text(r, "id").eq_ignore_ascii_case(&me)
                            && r["vote"].as_i64().unwrap_or(0) <= 0
                    })
            })
            .collect())
    }
    pub async fn pr_detail(&self, id: i32) -> Result<String> {
        let (_, detail) = self.pr_details(id).await?;
        Ok(detail)
    }
    pub async fn pr_details(&self, id: i32) -> Result<(Value, String)> {
        let pr = self.pr(id).await?;
        let repo = Self::repo_id(&pr)?;
        let threads = call!(self.git().pull_request_threads_client().list(
            self.org(),
            &repo,
            id,
            self.project()
        ))?;
        let statuses = call!(self.git().pull_request_statuses_client().list(
            self.org(),
            &repo,
            id,
            self.project()
        ))?;
        let project_id = pr["repository"]["project"]["id"]
            .as_str()
            .context("PR response has no project ID")?;
        let artifact = format!("vstfs:///CodeReview/CodeReviewId/{project_id}/{id}");
        let policies = call!(
            ado::policy::ClientBuilder::new(self.credential.clone())
                .endpoint(self.endpoint.clone())
                .retry(RetryOptions::none())
                .per_call_policies(policies(None))
                .build()
                .evaluations_client()
                .list(self.org(), self.project(), artifact)
        )?;
        let mut out = format!(
            "PR {id}\nTitle: {}\nState: {}\nDraft: {}\n{} -> {}\n{}\n\n{}\n\nReviewers\n",
            text(&pr, "title"),
            text(&pr, "status"),
            scalar(&pr["isDraft"]),
            text(&pr, "sourceRefName"),
            text(&pr, "targetRefName"),
            text(&pr, "remoteUrl"),
            text(&pr, "description")
        );
        for r in rows(&pr["reviewers"]) {
            out.push_str(&format!(
                "{}\t{}\n",
                text(&r, "displayName"),
                scalar(&r["vote"])
            ));
        }
        out.push_str("\nChecks\n");
        for s in rows(&statuses) {
            out.push_str(&format!(
                "{}\t{}\n",
                text(&s["context"], "name"),
                text(&s, "state")
            ));
        }
        out.push_str("\nPolicies\n");
        for p in rows(&policies) {
            out.push_str(&format!(
                "{}\t{}\n",
                text(&p["configuration"]["type"], "displayName"),
                text(&p, "status")
            ));
        }
        out.push_str("\nDiscussion\n");
        for thread in rows(&threads) {
            out.push_str(&format!("Thread {}\n", scalar(&thread["id"])));
            for comment in rows(&thread["comments"]) {
                if comment["isDeleted"].as_bool() != Some(true) {
                    out.push_str(&format!(
                        "{}: {}\n",
                        text(&comment["author"], "displayName"),
                        text(&comment, "content")
                    ));
                }
            }
        }
        let mut pr = pr;
        pr["detailThreads"] = threads;
        pr["detailStatuses"] = statuses;
        pr["detailPolicies"] = policies;
        Ok((pr, output::clean(&out)))
    }
    pub async fn pr_json(&self, command: &PrCommand) -> Result<Value> {
        command.validate()?;
        Ok(match command {
            PrCommand::List {
                state,
                repo,
                reviewer,
                author,
            } => json!(
                self.pr_list(
                    *state,
                    repo.as_deref(),
                    reviewer.as_deref(),
                    author.as_deref()
                )
                .await?
            ),
            PrCommand::Status => json!(self.attention_prs().await?),
            PrCommand::View { id } => self.pr(*id).await?,
            _ => json!({"message": self.pr_action(command).await?}),
        })
    }
    pub async fn pipeline_json(&self, command: &PipelineCommand) -> Result<Value> {
        command.validate()?;
        Ok(match command {
            PipelineCommand::List => json!(self.pipeline_list().await?),
            PipelineCommand::Rename { id, name } => {
                self.pipeline_edit(*id, Some(name), None).await?
            }
            PipelineCommand::Move { id, folder } => {
                self.pipeline_edit(*id, None, Some(folder)).await?
            }
            PipelineCommand::View { id } => self.require_yaml(*id).await?,
            PipelineCommand::Runs { id } => json!(self.runs(*id).await?),
            PipelineCommand::Inspect { run } => {
                json!({"run": self.build_run(*run).await?, "timeline": self.timeline(*run).await?})
            }
            PipelineCommand::Logs { run, log, start } => {
                self.build_run(*run).await?;
                json!(self.log(*run, *log, *start).await?)
            }
            _ => json!({"message": self.pipeline_action(command).await?}),
        })
    }
    pub async fn pr_action(&self, command: &PrCommand) -> Result<String> {
        command.validate()?;
        match command {
            PrCommand::List {
                state,
                repo,
                reviewer,
                author,
            } => pr_table(
                &self
                    .pr_list(
                        *state,
                        repo.as_deref(),
                        reviewer.as_deref(),
                        author.as_deref(),
                    )
                    .await?,
            ),
            PrCommand::Status => pr_table(&self.attention_prs().await?),
            PrCommand::View { id } => self.pr_detail(*id).await,
            PrCommand::Create(args) => {
                let mut body = git::GitPullRequestCreateOptions::new(
                    branch(&args.source),
                    branch(&args.target),
                    args.title.clone(),
                );
                body.description = Some(args.description.clone());
                body.is_draft = Some(args.draft);
                body.reviewers = args
                    .reviewer
                    .iter()
                    .map(|id| git::IdentityId::new(id.clone()))
                    .collect();
                let pr = call!(self.git().pull_requests_client().create(
                    self.org(),
                    segment(&args.repo),
                    self.project(),
                    body
                ))?;
                Ok(pr_result(&pr))
            }
            PrCommand::Edit(args) => {
                let pr = self.pr(args.id).await?;
                let repo = Self::repo_id(&pr)?;
                let body = git::GitPullRequestUpdateOptions {
                    title: args.title.clone(),
                    description: args.description.clone(),
                    ..Default::default()
                };
                if args.title.is_some() || args.description.is_some() || args.draft.is_some() {
                    let mut payload = serde_json::to_value(&body)?;
                    if let Some(draft) = args.draft {
                        payload["isDraft"] = json!(draft);
                    }
                    call!(self.git_body(Some(payload)).pull_requests_client().update(
                        self.org(),
                        &repo,
                        self.project(),
                        args.id,
                        body
                    ))?;
                }
                // Do not retry a partially completed multi-request edit automatically.
                for id in &args.add_reviewer {
                    call!(
                        self.git()
                            .pull_request_reviewers_client()
                            .create_pull_request_reviewers(
                                self.org(),
                                vec![git::IdentityRef::new(id.clone())],
                                &repo,
                                args.id,
                                self.project()
                            )
                    )?;
                }
                for id in &args.remove_reviewer {
                    call!(self.git().pull_request_reviewers_client().delete(
                        self.org(),
                        &repo,
                        args.id,
                        segment(id),
                        self.project()
                    ))?;
                }
                Ok(format!("Updated PR {}", args.id))
            }
            PrCommand::Comment { id, body } => {
                let pr = self.pr(*id).await?;
                let thread: git::GitPullRequestCommentThread = serde_json::from_value(
                    json!({"comments":[{"parentCommentId":0,"content":body,"commentType":"text"}],"status":"active"}),
                )?;
                call!(self.git().pull_request_threads_client().create(
                    self.org(),
                    thread,
                    Self::repo_id(&pr)?,
                    *id,
                    self.project()
                ))?;
                Ok(format!("Commented on PR {id}"))
            }
            PrCommand::Review { id, vote } => {
                let pr = self.pr(*id).await?;
                let id_me = self.current_user_id().await?;
                let reviewer: git::IdentityRefWithVote =
                    serde_json::from_value(json!({"id": id_me, "vote":vote.number()}))?;
                call!(
                    self.git()
                        .pull_request_reviewers_client()
                        .create_pull_request_reviewer(
                            self.org(),
                            reviewer,
                            Self::repo_id(&pr)?,
                            *id,
                            segment(&id_me),
                            self.project()
                        )
                )?;
                Ok(format!("Recorded review for PR {id}"))
            }
            PrCommand::Merge { id, strategy, .. } => {
                let pr = self.pr(*id).await?;
                ensure!(pr["status"] == "active", "Only active PRs can be completed");
                let commit = pr["lastMergeSourceCommit"]["commitId"]
                    .as_str()
                    .context("Cannot merge without the latest source commit")?;
                let payload = json!({"status":"completed", "lastMergeSourceCommit":{"commitId":commit}, "completionOptions":{"mergeStrategy":strategy.api(),"bypassPolicy":false,"deleteSourceBranch":false,"transitionWorkItems":false}});
                let body: git::GitPullRequestUpdateOptions =
                    serde_json::from_value(payload.clone())?;
                let response = call!(self.git_body(Some(payload)).pull_requests_client().update(
                    self.org(),
                    Self::repo_id(&pr)?,
                    self.project(),
                    *id,
                    body
                ))?;
                Ok(format!(
                    "Completion requested for PR {id}\nState: {}\nMerge status: {}",
                    text(&response, "status"),
                    text(&response, "mergeStatus")
                ))
            }
            PrCommand::Abandon { id, .. } | PrCommand::Reactivate { id } => {
                let pr = self.pr(*id).await?;
                let state = if matches!(command, PrCommand::Abandon { .. }) {
                    "abandoned"
                } else {
                    "active"
                };
                let body = serde_json::from_value::<git::GitPullRequestUpdateOptions>(
                    json!({"status":state}),
                )?;
                call!(self.git().pull_requests_client().update(
                    self.org(),
                    Self::repo_id(&pr)?,
                    self.project(),
                    *id,
                    body
                ))?;
                Ok(format!("PR {id}: {state}"))
            }
        }
    }

    pub async fn pipeline_list(&self) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut token = None;
        loop {
            let mut request = self
                .pipelines(None)
                .pipelines_client()
                .list(self.org(), self.project())
                .top(100);
            if let Some(t) = token.as_ref() {
                request = request.continuation_token(t);
            }
            let response = send(request.send()).await?.into_raw_response();
            let next = continuation(&response);
            // The pipeline API can include designer pipelines. Verify configuration before exposing them.
            for item in rows(&decode(response)?) {
                let detail = self.pipeline(response_id(&item, "id")?).await?;
                if detail["configuration"]["type"] == "yaml" {
                    all.push(detail);
                }
            }
            if next.is_none() || next == token {
                break;
            }
            token = next;
        }
        Ok(all)
    }
    pub async fn pipeline(&self, id: i32) -> Result<Value> {
        positive(id)?;
        call!(
            self.pipelines(None)
                .pipelines_client()
                .get(self.org(), self.project(), id)
        )
    }
    pub async fn require_yaml(&self, id: i32) -> Result<Value> {
        let pipeline = self.pipeline(id).await?;
        ensure!(
            pipeline["configuration"]["type"] == "yaml",
            "Only YAML pipelines are supported"
        );
        Ok(pipeline)
    }
    pub async fn recent_runs(&self, pipelines: &[Value]) -> Result<Vec<Value>> {
        let definitions = pipelines
            .iter()
            .map(|p| response_id(p, "id").map(|id| id.to_string()))
            .collect::<Result<Vec<_>>>()?;
        if definitions.is_empty() {
            return Ok(Vec::new());
        }
        Ok(rows(&call!(
            self.build(None)
                .builds_client()
                .list(self.org(), self.project())
                .definitions(definitions.join(","))
                .top(50)
                .query_order(ado::build::models::BuildQueryOrder::QueueTimeDescending)
        )?))
    }
    pub async fn runs(&self, id: i32) -> Result<Vec<Value>> {
        self.require_yaml(id).await?;
        Ok(rows(&call!(self.pipelines(None).runs_client().list(
            self.org(),
            self.project(),
            id
        ))?))
    }
    pub async fn build_run(&self, id: i32) -> Result<Value> {
        positive(id)?;
        let run = call!(
            self.build(None)
                .builds_client()
                .get(self.org(), self.project(), id)
        )?;
        let pipeline_id = response_id(&run["definition"], "id")?;
        self.require_yaml(pipeline_id).await?;
        Ok(run)
    }
    pub async fn timeline(&self, run: i32) -> Result<Vec<Value>> {
        // An empty timeline ID intentionally selects the build's current timeline.
        let result = send(
            self.build(None)
                .timeline_client()
                .get(self.org(), self.project(), run, "")
                .send(),
        )
        .await;
        match result {
            Ok(response) => Ok(rows(&decode(response.into_raw_response())?["records"])),
            // Queued runs may not have a timeline yet. The caller verifies the run exists.
            Err(error)
                if error
                    .downcast_ref::<error::HttpFailure>()
                    .is_some_and(|e| e.0 == 404) =>
            {
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }
    pub fn live(&self) -> crate::live::Client {
        crate::live::Client::new(self.credential.clone(), self.target.organization.clone())
    }
    pub async fn log(&self, run: i32, log: i32, start: u64) -> Result<Vec<String>> {
        positive(run)?;
        positive(log)?;
        ensure!(start <= i64::MAX as u64 - 1999, "Log offset too large");
        let response = send(
            self.build(None)
                .builds_client()
                .get_build_log(self.org(), self.project(), run, log)
                .start_line(start as i64)
                .end_line(start as i64 + 1999)
                .send(),
        )
        .await?
        .into_raw_response();
        let body = response.into_body();
        // Some service versions return a JSON list; others return plain text.
        if let Ok(value) = serde_json::from_slice::<Value>(body.as_ref())
            && (value.is_array() || value["value"].is_array())
        {
            return Ok(rows(&value)
                .iter()
                .map(|v| output::clean(v.as_str().unwrap_or("")))
                .collect());
        }
        let body = std::str::from_utf8(body.as_ref()).context("Invalid log encoding")?;
        Ok(body.lines().map(output::clean).collect())
    }
    pub async fn pipeline_action(&self, command: &PipelineCommand) -> Result<String> {
        command.validate()?;
        match command {
            PipelineCommand::Rename { id, name } => {
                self.pipeline_edit(*id, Some(name), None).await?;
                Ok(format!("Renamed pipeline {id} to {name}"))
            }
            PipelineCommand::Move { id, folder } => {
                self.pipeline_edit(*id, None, Some(folder)).await?;
                Ok(format!("Moved pipeline {id} to {folder}"))
            }
            PipelineCommand::List => output::table(
                &self.pipeline_list().await?,
                &[("NAME", "name"), ("FOLDER", "folder")],
            ),
            PipelineCommand::View { id } => {
                let p = self.require_yaml(*id).await?;
                Ok(format!(
                    "Pipeline {id}\nName: {}\nFolder: {}\nYAML: {}\nRepository: {}",
                    text(&p, "name"),
                    text(&p, "folder"),
                    text(&p["configuration"], "path"),
                    text(&p["configuration"]["repository"], "id")
                ))
            }
            PipelineCommand::Runs { id } => run_table(&self.runs(*id).await?),
            PipelineCommand::Inspect { run } => {
                let build = self.build_run(*run).await?;
                let timeline = self.timeline(*run).await?;
                Ok(format!(
                    "{}\n\n{}",
                    build_summary(&build),
                    output::table(
                        &timeline,
                        &[
                            ("ID", "id"),
                            ("PARENT", "parentId"),
                            ("TYPE", "type"),
                            ("NAME", "name"),
                            ("STATE", "state"),
                            ("RESULT", "result"),
                            ("PERCENT", "percentComplete")
                        ]
                    )?
                ))
            }
            PipelineCommand::Logs { run, log, start } => {
                self.build_run(*run).await?;
                Ok(self.log(*run, *log, *start).await?.join("\n"))
            }
            PipelineCommand::Run {
                id,
                branch: reference,
                param,
                variable,
            } => {
                self.require_yaml(*id).await?;
                let mut body = ado::pipelines::models::RunPipelineParameters {
                    template_parameters: Some(Value::Object(pairs(param, true)?)),
                    ..Default::default()
                };
                let vars = pairs(variable, false)?;
                body.variables = Some(Value::Object(
                    vars.into_iter()
                        .map(|(k, v)| (k, json!({"value":v,"isSecret":false})))
                        .collect(),
                ));
                if let Some(reference) = reference {
                    body.resources = Some(serde_json::from_value(
                        json!({"repositories":{"self":{"refName":branch(reference)}}}),
                    )?);
                }
                let run = call!(self.pipelines(None).runs_client().run_pipeline(
                    self.org(),
                    body,
                    self.project(),
                    *id
                ))?;
                Ok(run_summary(&run))
            }
            PipelineCommand::Cancel { run, .. } => {
                self.build_run(*run).await?;
                self.patch_build(*run, false, json!({"status":"cancelling"}))
                    .await?;
                Ok(format!("Cancellation requested for run {run}"))
            }
            PipelineCommand::Retry { run } => {
                let build = self.build_run(*run).await?;
                ensure!(
                    build["status"] == "completed",
                    "Only completed runs can be retried"
                );
                self.patch_build(*run, true, json!({"status":"inProgress"}))
                    .await?;
                Ok(format!(
                    "Retry requested for run {run}. The server chooses eligible failed work."
                ))
            }
            PipelineCommand::Rerun { id, run } => {
                self.require_yaml(*id).await?;
                let previous = call!(self.pipelines(None).runs_client().get(
                    self.org(),
                    self.project(),
                    *id,
                    *run
                ))?;
                ensure!(
                    previous["pipeline"]["id"] == *id,
                    "Run does not belong to this pipeline"
                );
                let variables = previous["variables"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                ensure!(
                    !variables.values().any(|v| v["isSecret"] == true),
                    "Cannot safely copy secret run overrides. Trigger an explicit new run instead."
                );
                let mut body = ado::pipelines::models::RunPipelineParameters {
                    template_parameters: previous.get("templateParameters").cloned(),
                    variables: Some(Value::Object(variables)),
                    ..Default::default()
                };
                // Reuse references and pinned commits, never repository bearer tokens.
                if let Some(resources) = previous["resources"]["repositories"].as_object() {
                    let repositories: serde_json::Map<String, Value> = resources
                        .iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                json!({"refName":v["refName"],"version":v["version"]}),
                            )
                        })
                        .collect();
                    body.resources = Some(serde_json::from_value(
                        json!({"repositories":repositories}),
                    )?);
                }
                ensure!(
                    previous["resources"].as_object().is_none_or(|r| r
                        .iter()
                        .all(|(k, v)| k == "repositories"
                            || v.is_null()
                            || v.as_object().is_some_and(|o| o.is_empty()))),
                    "Rerun of external resources requires an explicit run. Inputs will not be silently lost."
                );
                let new_run = call!(self.pipelines(None).runs_client().run_pipeline(
                    self.org(),
                    body,
                    self.project(),
                    *id
                ))?;
                Ok(run_summary(&new_run))
            }
            PipelineCommand::Register { name, repo, yaml } => {
                ensure!(
                    !name.trim().is_empty() && !yaml.trim().is_empty(),
                    "Pipeline name and committed YAML path are required"
                );
                let repository = call!(self.git().repositories_client().get_repository(
                    self.org(),
                    segment(repo),
                    self.project()
                ))?;
                let payload = json!({"name":name,"configuration":{"type":"yaml","path":yaml,"repository":{"id":repository["id"],"type":"azureReposGit"}}});
                let body = ado::pipelines::models::CreatePipelineParameters {
                    name: Some(name.clone()),
                    configuration: Some(serde_json::from_value(json!({"type":"yaml"}))?),
                    ..Default::default()
                };
                let pipeline = call!(self.pipelines(Some(payload)).pipelines_client().create(
                    self.org(),
                    body,
                    self.project()
                ))?;
                Ok(format!(
                    "Registered pipeline {}\n{}",
                    scalar(&pipeline["id"]),
                    text(&pipeline, "name")
                ))
            }
            PipelineCommand::Watch { .. } => {
                anyhow::bail!("Watch is handled by the command runner")
            }
        }
    }
    async fn patch_build(&self, id: i32, retry: bool, payload: Value) -> Result<Value> {
        let body: ado::build::models::Build = serde_json::from_value(
            json!({"id":id,"definition":{"id":0,"project":{"name":"","visibility":"private"},"queueStatus":"enabled","revision":0,"url":""},"project":{"name":"","visibility":"private"},"priority":"normal","reason":"manual"}),
        )?;
        call!(
            self.build(Some(payload))
                .builds_client()
                .update_build(self.org(), body, self.project(), id)
                .retry(retry)
        )
    }
}

fn continuation(response: &RawResponse) -> Option<String> {
    response
        .headers()
        .get_optional_string(&"x-ms-continuationtoken".into())
        .filter(|s| !s.is_empty())
}
pub fn response_id(value: &Value, key: &str) -> Result<i32> {
    value[key]
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .filter(|id| *id > 0)
        .with_context(|| format!("Azure DevOps response has no valid {key}"))
}
fn pairs(input: &[String], typed: bool) -> Result<serde_json::Map<String, Value>> {
    let mut map = serde_json::Map::new();
    for pair in input {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| error::invalid("Expected NAME=VALUE"))?;
        ensure!(
            !key.is_empty() && !key.chars().any(char::is_control),
            "Invalid parameter or variable name"
        );
        let value = if typed {
            serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.into()))
        } else {
            Value::String(value.into())
        };
        ensure!(
            map.insert(key.into(), value).is_none(),
            "Duplicate parameter or variable name"
        );
    }
    Ok(map)
}
pub fn pr_table(items: &[Value]) -> Result<String> {
    output::table(
        items,
        &[
            ("ID", "pullRequestId"),
            ("STATE", "status"),
            ("DRAFT", "isDraft"),
            ("TITLE", "title"),
            ("SOURCE", "sourceRefName"),
            ("TARGET", "targetRefName"),
        ],
    )
}
fn pr_result(pr: &Value) -> String {
    format!(
        "PR {}\n{}",
        scalar(&pr["pullRequestId"]),
        text(pr, "remoteUrl")
    )
}
pub fn run_table(items: &[Value]) -> Result<String> {
    output::table(
        items,
        &[
            ("ID", "id"),
            ("NAME", "name"),
            ("STATE", "state"),
            ("RESULT", "result"),
            ("CREATED", "createdDate"),
        ],
    )
}
pub fn run_summary(run: &Value) -> String {
    format!(
        "Run {}\nState: {}\nResult: {}",
        scalar(&run["id"]),
        text(run, "state"),
        text(run, "result")
    )
}
pub fn build_summary(run: &Value) -> String {
    format!(
        "Run {}\nPipeline: {}\nBranch: {}\nStatus: {}\nResult: {}",
        scalar(&run["id"]),
        text(&run["definition"], "name"),
        text(run, "sourceBranch"),
        text(run, "status"),
        text(run, "result")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path, query_param},
    };

    #[derive(Debug)]
    struct TestCredential;
    #[async_trait::async_trait]
    impl azure_core::credentials::TokenCredential for TestCredential {
        async fn get_token(
            &self,
            scopes: &[&str],
            _: Option<azure_core::credentials::TokenRequestOptions<'_>>,
        ) -> azure_core::Result<azure_core::credentials::AccessToken> {
            assert_eq!(scopes, [auth::SCOPE]);
            Ok(azure_core::credentials::AccessToken::new(
                "fixture-access-token",
                time::OffsetDateTime::now_utc() + time::Duration::hours(1),
            ))
        }
    }
    fn api(server: &MockServer) -> Result<Service> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let endpoint: azure_core::http::Url = server.uri().parse()?;
        Ok(Service {
            credential: ado::Credential::from_token_credential(Arc::new(TestCredential)),
            endpoint: endpoint.clone(),
            profile_endpoint: endpoint,
            target: Target {
                organization: "org".into(),
                project: "project".into(),
            },
        })
    }
    async fn mock_get(server: &MockServer, url: &str, value: Value) {
        Mock::given(method("GET"))
            .and(path(url))
            .respond_with(ResponseTemplate::new(200).set_body_json(value))
            .mount(server)
            .await;
    }
    #[tokio::test]
    async fn pat_review_uses_organization_identity_and_sends_each_vote() -> Result<()> {
        let server = MockServer::start().await;
        let mut api = api(&server)?;
        api.credential = ado::Credential::from_pat("fixture-pat");
        let me = "11111111-1111-1111-1111-111111111111";
        mock_get(
            &server,
            "/org/project/_apis/git/pullrequests/7",
            json!({"pullRequestId":7,"repository":{"id":"repo"}}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/org/_apis/connectionData"))
            .and(header("authorization", "Basic OmZpeHR1cmUtcGF0"))
            .and(header("x-tfs-fedauthredirect", "Suppress"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"authenticatedUser":{"id":me}})),
            )
            .expect(5)
            .mount(&server)
            .await;
        Mock::given(path("/_apis/profile/profiles/me"))
            .respond_with(ResponseTemplate::new(401))
            .expect(0)
            .mount(&server)
            .await;
        for vote in [
            Vote::Approve,
            Vote::ApproveWithSuggestions,
            Vote::Wait,
            Vote::Reject,
            Vote::Reset,
        ] {
            Mock::given(method("PUT"))
                .and(path(format!(
                    "/org/project/_apis/git/repositories/repo/pullRequests/7/reviewers/{}",
                    segment(me)
                )))
                .and(wiremock::matchers::body_partial_json(
                    json!({"id":me,"vote":vote.number()}),
                ))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(json!({"id":me,"vote":vote.number()})),
                )
                .expect(1)
                .mount(&server)
                .await;
            assert!(
                api.pr_action(&PrCommand::Review { id: 7, vote })
                    .await?
                    .contains("Recorded review")
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn assigned_reviews_use_org_identity_and_filter_drafts_and_positive_votes() -> Result<()>
    {
        let server = MockServer::start().await;
        let me = "11111111-1111-1111-1111-111111111111";
        mock_get(
            &server,
            "/org/_apis/connectionData",
            json!({"authenticatedUser":{"id":me}}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/git/pullrequests"))
            .and(query_param("searchCriteria.reviewerId", me))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[
                {"pullRequestId":1,"reviewers":[{"id":me,"vote":0}]},
                {"pullRequestId":2,"isDraft":true,"reviewers":[{"id":me,"vote":0}]},
                {"pullRequestId":3,"reviewers":[{"id":me,"vote":10}]},
                {"pullRequestId":4,"reviewers":[{"id":me,"vote":-5}]}
            ]})))
            .expect(1)
            .mount(&server)
            .await;
        let prs = api(&server)?.attention_prs().await?;
        assert_eq!(
            prs.iter()
                .map(|p| p["pullRequestId"].as_i64())
                .collect::<Vec<_>>(),
            vec![Some(1), Some(4)]
        );
        Ok(())
    }

    #[tokio::test]
    async fn identity_errors_preserve_auth_codes_without_response_secrets() -> Result<()> {
        for status in [401, 403, 302] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/org/_apis/connectionData"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .set_body_string("private-response")
                        .insert_header("location", "https://example.invalid/secret"),
                )
                .expect(1)
                .mount(&server)
                .await;
            let error = api(&server)?
                .current_user_id()
                .await
                .err()
                .context("Expected identity error")?;
            assert!(!error.to_string().contains("private-response"));
            if status != 302 {
                assert_eq!(
                    error
                        .downcast_ref::<error::Failure>()
                        .context("Expected stable failure")?
                        .code,
                    if status == 401 { 3 } else { 4 }
                );
            }
        }
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/_apis/connectionData",
            json!({"authenticatedUser":{"id":"not-an-id"}}),
        )
        .await;
        assert!(api(&server)?.current_user_id().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn review_files_page_against_the_latest_iterations_merge_base() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(&server, "/org/project/_apis/git/repositories/repo/pullRequests/7/iterations",
            json!({"value":[{"id":2,"commonRefCommit":{"commitId":"base"},"sourceRefCommit":{"commitId":"head"}},{"id":1}]})).await;
        for (skip, response) in [
            (
                0,
                json!({"changeEntries":[{"item":{"path":"/one"},"changeType":"edit"},{"item":{"isFolder":true,"path":"/dir"}}],"nextSkip":2}),
            ),
            (
                2,
                json!({"changeEntries":[{"item":{"path":"/two"},"changeType":"add"}],"nextSkip":0}),
            ),
        ] {
            Mock::given(method("GET"))
                .and(path(
                    "/org/project/_apis/git/repositories/repo/pullRequests/7/iterations/2/changes",
                ))
                .and(query_param("$compareTo", "0"))
                .and(query_param("$skip", skip.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .expect(1)
                .mount(&server)
                .await;
        }
        let changes = api(&server)?
            .pr_changes(&json!({"pullRequestId":7,"repository":{"id":"repo"}}))
            .await?;
        assert_eq!(changes["base"], "base");
        assert_eq!(changes["head"], "head");
        assert_eq!(changes["files"].as_array().context("Files")?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn review_diff_handles_add_delete_rename_binary_and_missing_content() -> Result<()> {
        for (kind, old_path, old, new, expected) in [
            (
                "add",
                "/file",
                json!({"content":""}),
                json!({"content":"added\n"}),
                "+added",
            ),
            (
                "delete",
                "/file",
                json!({"content":"removed\n"}),
                json!({"content":""}),
                "-removed",
            ),
            (
                "rename, edit",
                "/old name",
                json!({"content":"before\n"}),
                json!({"content":"after\n"}),
                "--- a/old name",
            ),
            (
                "edit",
                "/file",
                json!({"contentMetadata":{"isBinary":true}}),
                json!({"content":""}),
                "Binary file",
            ),
            (
                "edit",
                "/file",
                json!({"content":""}),
                json!({"content":"x".repeat(1024*1024+1)}),
                "1 MiB",
            ),
        ] {
            let server = MockServer::start().await;
            for (commit, path_value, body, needed) in [
                ("base", old_path, old, kind != "add"),
                ("head", "/file", new, kind != "delete"),
            ] {
                Mock::given(method("GET"))
                    .and(path("/org/project/_apis/git/repositories/repo/items"))
                    .and(query_param("path", path_value))
                    .and(query_param("versionDescriptor.version", commit))
                    .and(query_param("versionDescriptor.versionType", "commit"))
                    .and(query_param("includeContent", "true"))
                    .respond_with(ResponseTemplate::new(200).set_body_json(body))
                    .expect(u64::from(needed))
                    .mount(&server)
                    .await;
            }
            let diff = api(&server)?
                .pr_file_diff(
                    &json!({"repository":"repo","base":"base","head":"head","files":[
                        {"changeType":kind,"originalPath":old_path,"item":{"path":"/file"}}
                    ]}),
                    0,
                )
                .await?;
            assert!(diff.contains(expected), "{diff}");
            if kind == "rename, edit" {
                assert!(diff.contains("-before") && diff.contains("+after"));
            }
        }
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/git/repositories/repo/items",
            json!({}),
        )
        .await;
        assert!(api(&server)?.pr_file_diff(&json!({"repository":"repo","base":"base","head":"head","files":[{"changeType":"edit","item":{"path":"/file"}}]}),0).await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn repository_list_returns_project_repositories() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/git/repositories",
            json!({"value":[{"id":"repo","name":"demo"}]}),
        )
        .await;
        assert_eq!(api(&server)?.repo_list().await?[0]["name"], "demo");
        Ok(())
    }
    #[tokio::test]
    async fn pipeline_edit_preserves_definition_and_revision() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/pipelines/7",
            json!({"id":7,"configuration":{"type":"yaml"}}),
        )
        .await;
        let mut definition = json!({"id":7,"name":"old","path":"\\","revision":4,"queueStatus":"enabled","url":"","project":{"name":"project","visibility":"private"},"process":{"type":2,"yamlFilename":"ci.yml"},"variables":{"key":{"value":"keep"}}});
        mock_get(
            &server,
            "/org/project/_apis/build/definitions/7",
            definition.clone(),
        )
        .await;
        definition["name"] = json!("new");
        definition["path"] = json!("\\team\\ci");
        Mock::given(method("PUT"))
            .and(path("/org/project/_apis/build/definitions/7"))
            .and(body_json(definition.clone()))
            .respond_with(ResponseTemplate::new(200).set_body_json(definition.clone()))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)?
                .pipeline_edit(7, Some("new"), Some("team/ci"))
                .await?,
            definition
        );
        Ok(())
    }
    fn pr_fixture() -> Value {
        json!({"pullRequestId":7,"status":"active","repository":{"id":"repo","project":{"id":"project-id"}},"lastMergeSourceCommit":{"commitId":"expected-head"}})
    }
    async fn mock_run(server: &MockServer) {
        mock_get(
            server,
            "/org/project/_apis/build/builds/9",
            json!({"id":9,"status":"completed","definition":{"id":2}}),
        )
        .await;
        mock_get(
            server,
            "/org/project/_apis/pipelines/2",
            json!({"id":2,"configuration":{"type":"yaml"}}),
        )
        .await;
    }
    #[tokio::test]
    async fn creates_only_an_empty_repository() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/org/project/_apis/git/repositories")).and(header("authorization","Bearer fixture-access-token"))
            .and(body_json(json!({"name":"demo"}))).respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":"repo","name":"demo","remoteUrl":"https://dev.azure.com/org/project/_git/demo"}))).expect(1).mount(&server).await;
        assert_eq!(api(&server)?.repo_create("demo").await?["name"], "demo");
        Ok(())
    }

    #[tokio::test]
    async fn recent_runs_are_bounded_and_ordered_for_selected_definitions() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/build/builds"))
            .and(query_param("definitions", "2,7"))
            .and(query_param("$top", "50"))
            .and(query_param("queryOrder", "queueTimeDescending"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[{"id":42}]})))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)?
                .recent_runs(&[json!({"id":2}), json!({"id":7})])
                .await?[0]["id"],
            42
        );
        assert!(api(&server)?.recent_runs(&[]).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn pipeline_json_returns_array_without_table_wrapping() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/pipelines"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[]})))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)?.pipeline_json(&PipelineCommand::List).await?,
            json!([])
        );
        Ok(())
    }

    #[tokio::test]
    async fn pat_login_suppresses_browser_redirects() -> Result<()> {
        let server = MockServer::start().await;
        let mut api = api(&server)?;
        api.credential = auth::Credential::from_pat("fixture-pat".into())?.sdk();
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "https://example.invalid/signin"),
            )
            .with_priority(10)
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("x-tfs-fedauthredirect", "Suppress"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        let failure = api
            .check_access()
            .await
            .err()
            .context("Expected authentication failure")?;
        assert_eq!(
            failure
                .downcast_ref::<error::Failure>()
                .context("Expected stable authentication error")?
                .code,
            3
        );
        Ok(())
    }

    #[tokio::test]
    async fn pat_uses_basic_auth_for_project_validation_and_operations() -> Result<()> {
        use base64::Engine;
        let server = MockServer::start().await;
        let mut api = api(&server)?;
        api.credential = auth::Credential::from_pat("fixture-pat".into())?.sdk();
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(":fixture-pat")
        );
        Mock::given(method("GET"))
            .and(path("/org/_apis/projects"))
            .and(query_param("$top", "1"))
            .and(header("authorization", authorization.as_str()))
            .and(header("x-tfs-fedauthredirect", "Suppress"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[]})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/pipelines"))
            .and(header("authorization", authorization.as_str()))
            .and(header("x-tfs-fedauthredirect", "Suppress"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[]})))
            .expect(1)
            .mount(&server)
            .await;
        api.check_access().await?;
        assert!(api.pipeline_list().await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn pat_failures_keep_stable_exit_codes_and_do_not_echo_responses() -> Result<()> {
        for (status, code) in [(401, 3), (403, 4)] {
            let server = MockServer::start().await;
            let mut api = api(&server)?;
            api.credential = auth::Credential::from_pat("fixture-pat".into())?.sdk();
            Mock::given(method("GET"))
                .and(path("/org/_apis/projects"))
                .respond_with(ResponseTemplate::new(status).set_body_string("fixture-pat"))
                .expect(1)
                .mount(&server)
                .await;
            let error = api
                .check_access()
                .await
                .err()
                .context("Expected PAT failure")?;
            assert_eq!(
                error
                    .downcast_ref::<error::Failure>()
                    .context("Expected stable exit code")?
                    .code,
                code
            );
            assert!(!error.to_string().contains("fixture-pat"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn pat_validation_rejects_sign_in_html_instead_of_accepting_it_as_success() -> Result<()>
    {
        let server = MockServer::start().await;
        let mut api = api(&server)?;
        api.credential = auth::Credential::from_pat("fixture-pat".into())?.sdk();
        Mock::given(method("GET"))
            .and(path("/org/_apis/projects"))
            .respond_with(ResponseTemplate::new(203).set_body_string("<html>Sign in</html>"))
            .expect(1)
            .mount(&server)
            .await;
        assert!(api.check_access().await.is_err());
        Ok(())
    }
    #[tokio::test]
    async fn acknowledgement_fails_before_any_requests() -> Result<()> {
        let server = MockServer::start().await;
        assert!(
            api(&server)?
                .pr_action(&PrCommand::Abandon { id: 7, yes: false })
                .await
                .is_err()
        );
        assert!(
            api(&server)?
                .pipeline_action(&PipelineCommand::Cancel { run: 9, yes: false })
                .await
                .is_err()
        );
        assert!(
            server
                .received_requests()
                .await
                .context("Request recording disabled")?
                .is_empty()
        );
        Ok(())
    }
    #[tokio::test]
    async fn merge_preserves_expected_head_and_never_bypasses_policy() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/git/pullrequests/7",
            pr_fixture(),
        )
        .await;
        Mock::given(method("PATCH")).and(path("/org/project/_apis/git/repositories/repo/pullrequests/7"))
            .and(body_json(json!({"status":"completed","lastMergeSourceCommit":{"commitId":"expected-head"},"completionOptions":{"mergeStrategy":"squash","bypassPolicy":false,"deleteSourceBranch":false,"transitionWorkItems":false}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"status":"active","mergeStatus":"queued"}))).expect(1).mount(&server).await;
        let out = api(&server)?
            .pr_action(&PrCommand::Merge {
                id: 7,
                strategy: MergeStrategy::Squash,
                yes: true,
            })
            .await?;
        assert!(out.contains("Completion requested"));
        assert!(!out.contains("fixture-access-token"));
        Ok(())
    }
    #[tokio::test]
    async fn edits_draft_without_clearing_other_fields() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/git/pullrequests/7",
            pr_fixture(),
        )
        .await;
        Mock::given(method("PATCH"))
            .and(path(
                "/org/project/_apis/git/repositories/repo/pullrequests/7",
            ))
            .and(body_json(json!({"isDraft":false})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        api(&server)?
            .pr_action(&PrCommand::Edit(PrEdit {
                id: 7,
                title: None,
                description: None,
                draft: Some(false),
                add_reviewer: vec![],
                remove_reviewer: vec![],
            }))
            .await?;
        Ok(())
    }
    #[tokio::test]
    async fn register_keeps_yaml_path_and_repository_in_sdk_payload() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/git/repositories/repo",
            json!({"id":"repo"}),
        )
        .await;
        Mock::given(method("POST")).and(path("/org/project/_apis/pipelines"))
            .and(body_json(json!({"name":"ci","configuration":{"type":"yaml","path":"/azure-pipelines.yml","repository":{"id":"repo","type":"azureReposGit"}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":2,"name":"ci"}))).expect(1).mount(&server).await;
        api(&server)?
            .pipeline_action(&PipelineCommand::Register {
                name: "ci".into(),
                repo: "repo".into(),
                yaml: "/azure-pipelines.yml".into(),
            })
            .await?;
        Ok(())
    }
    #[tokio::test]
    async fn cancellation_is_a_sparse_patch() -> Result<()> {
        let server = MockServer::start().await;
        mock_run(&server).await;
        Mock::given(method("PATCH"))
            .and(path("/org/project/_apis/build/builds/9"))
            .and(query_param("retry", "false"))
            .and(body_json(json!({"status":"cancelling"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        api(&server)?
            .pipeline_action(&PipelineCommand::Cancel { run: 9, yes: true })
            .await?;
        Ok(())
    }
    #[tokio::test]
    async fn retry_uses_sdk_retry_query() -> Result<()> {
        let server = MockServer::start().await;
        mock_run(&server).await;
        Mock::given(method("PATCH"))
            .and(path("/org/project/_apis/build/builds/9"))
            .and(query_param("retry", "true"))
            .and(body_json(json!({"status":"inProgress"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .expect(1)
            .mount(&server)
            .await;
        api(&server)?
            .pipeline_action(&PipelineCommand::Retry { run: 9 })
            .await?;
        Ok(())
    }
    #[tokio::test]
    async fn triggers_with_typed_parameters_and_non_secret_variables() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/pipelines/2",
            json!({"configuration":{"type":"yaml"}}),
        )
        .await;
        Mock::given(method("POST")).and(path("/org/project/_apis/pipelines/2/runs"))
            .and(body_json(json!({"templateParameters":{"deploy":true,"count":2},"variables":{"mode":{"value":"test","isSecret":false}},"resources":{"repositories":{"self":{"refName":"refs/heads/main"}}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":9,"state":"inProgress"}))).expect(1).mount(&server).await;
        let out = api(&server)?
            .pipeline_action(&PipelineCommand::Run {
                id: 2,
                branch: Some("main".into()),
                param: vec!["deploy=true".into(), "count=2".into()],
                variable: vec!["mode=test".into()],
            })
            .await?;
        assert!(!out.contains("mode"));
        Ok(())
    }
    #[tokio::test]
    async fn refuses_classic_pipeline_writes() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/org/project/_apis/pipelines/2",
            json!({"configuration":{"type":"designerJson"}}),
        )
        .await;
        assert!(
            api(&server)?
                .pipeline_action(&PipelineCommand::Run {
                    id: 2,
                    branch: None,
                    param: vec![],
                    variable: vec![]
                })
                .await
                .is_err()
        );
        assert_eq!(
            server
                .received_requests()
                .await
                .context("Request recording disabled")?
                .len(),
            1
        );
        Ok(())
    }
    #[tokio::test]
    async fn log_ranges_preserve_line_offsets_and_sanitize_text() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/build/builds/9/logs/3"))
            .and(query_param("startLine", "2000"))
            .and(query_param("endLine", "3999"))
            .respond_with(ResponseTemplate::new(200).set_body_string("hello\x1b[31m\r\nworld\n"))
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)?.log(9, 3, 2000).await?,
            vec!["hello[31m", "world"]
        );
        Ok(())
    }
    #[tokio::test]
    async fn errors_do_not_expose_response_bodies_or_retry_writes() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/org/project/_apis/git/repositories"))
            .respond_with(
                ResponseTemplate::new(503).set_body_string("fixture-access-token sensitive-body"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let message = api(&server)?
            .repo_create("demo")
            .await
            .err()
            .context("Expected service failure")?
            .to_string();
        assert!(message.contains("503"));
        assert!(!message.contains("sensitive-body"));
        assert!(!message.contains("fixture-access-token"));
        Ok(())
    }
    #[tokio::test]
    async fn project_listing_follows_continuation_tokens() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/_apis/projects"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-ms-continuationtoken", "100")
                    .set_body_json(json!({"value":[{"name":"one"}]})),
            )
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/org/_apis/projects"))
            .and(query_param("continuationToken", "100"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"value":[{"name":"two"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let projects = api(&server)?.projects().await?;
        assert_eq!(projects.len(), 2);
        Ok(())
    }
    #[tokio::test]
    async fn discovers_organizations_using_the_current_profile() -> Result<()> {
        let server = MockServer::start().await;
        mock_get(
            &server,
            "/_apis/profile/profiles/me",
            json!({"id":"member-id"}),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/_apis/accounts"))
            .and(query_param("memberId", "member-id"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"value":[{"accountName":"org"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            api(&server)?.organizations().await?[0]["accountName"],
            "org"
        );
        Ok(())
    }
    #[tokio::test]
    async fn queued_run_without_timeline_returns_an_empty_sidebar() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/build/builds/9/timeline/"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;
        assert!(api(&server)?.timeline(9).await?.is_empty());
        Ok(())
    }
    #[tokio::test]
    async fn project_names_are_encoded_once_in_real_requests() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/org/A%20Project/_apis/git/repositories"))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({"name":"demo"})))
            .expect(1)
            .mount(&server)
            .await;
        let mut api = api(&server)?;
        api.target.project = "A Project".into();
        api.repo_create("demo").await?;
        Ok(())
    }
    #[tokio::test]
    async fn permission_errors_have_a_stable_exit_code() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(403).set_body_string("sensitive denial"))
            .mount(&server)
            .await;
        let error = api(&server)?
            .repo_create("demo")
            .await
            .err()
            .context("Expected denial")?;
        assert_eq!(
            error
                .downcast_ref::<error::Failure>()
                .context("Expected permission failure")?
                .code,
            4
        );
        assert!(!format!("{error:?}").contains("sensitive denial"));
        Ok(())
    }
    #[test]
    fn parameters_reject_duplicates_and_path_segments_are_encoded() {
        assert!(pairs(&["x=1".into(), "x=2".into()], true).is_err());
        assert!(pairs(&["missing-separator".into()], true).is_err());
        assert_eq!(
            segment("A Project/../?x=y"),
            "A%20Project%2F%2E%2E%2F%3Fx%3Dy"
        );
    }

    #[test]
    fn response_ids_cannot_wrap_or_target_another_resource() -> Result<()> {
        assert_eq!(response_id(&json!({"id":42}), "id")?, 42);
        for value in [
            Value::Null,
            json!({}),
            json!({"id":"42"}),
            json!({"id":0}),
            json!({"id":-1}),
            json!({"id":4_294_967_297_i64}),
        ] {
            assert!(response_id(&value, "id").is_err());
        }
        Ok(())
    }

    #[tokio::test]
    async fn pr_details_reuse_a_single_pr_snapshot() -> Result<()> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/git/pullrequests/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(pr_fixture()))
            .expect(1)
            .mount(&server)
            .await;
        for url in [
            "/org/project/_apis/git/repositories/repo/pullRequests/7/threads",
            "/org/project/_apis/git/repositories/repo/pullRequests/7/statuses",
            "/org/project/_apis/policy/evaluations",
        ] {
            mock_get(&server, url, json!({"value":[]})).await;
        }
        let (pr, detail) = api(&server)?.pr_details(7).await?;
        assert_eq!(pr["pullRequestId"], 7);
        assert!(detail.contains("PR 7"));
        Ok(())
    }
}
