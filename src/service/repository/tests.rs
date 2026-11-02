use super::super::tests::api;
use super::*;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method, path, query_param},
};

#[tokio::test]
async fn branch_history_encodes_slashes_and_older_pages_use_a_pinned_commit() -> Result<()> {
    let server = MockServer::start().await;
    for (version, kind, skip) in [("fix/2.6.1", "branch", "0"), ("head", "commit", "100")] {
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/git/repositories/repo/commits"))
            .and(query_param("searchCriteria.itemVersion.version", version))
            .and(query_param("searchCriteria.itemVersion.versionType", kind))
            .and(query_param(
                "searchCriteria.showOldestCommitsFirst",
                "false",
            ))
            .and(query_param("searchCriteria.$top", "100"))
            .and(query_param("searchCriteria.$skip", skip))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"value":[{"commitId":"head"}]})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let api = api(&server)?;
    assert_eq!(
        api.repo_commits("repo", "fix/2.6.1", false, 0).await?[0]["commitId"],
        "head"
    );
    api.repo_commits("repo", "head", true, 100).await?;
    Ok(())
}

#[tokio::test]
async fn refs_page_and_annotated_and_lightweight_tags_share_batched_targets() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/org/project/_apis/git/repositories/repo/refs"))
        .and(query_param("filter", "tags/"))
        .and(query_param("peelTags", "true"))
        .respond_with(|request: &wiremock::Request| {
            let next = request.url.query_pairs().any(|(key, value)| key == "continuationToken" && value == "page two");
            if next {
                ResponseTemplate::new(200).set_body_json(json!({"value":[
                    {"name":"refs/tags/fix/2.5.0","objectId":"shared","peeledObjectId":"0000000000000000000000000000000000000000"}
                ]}))
            } else {
                ResponseTemplate::new(200).insert_header("x-ms-continuationtoken", "page two").set_body_json(json!({"value":[
                    {"name":"refs/tags/fix/2.6.1","objectId":"tag-object","peeledObjectId":"shared"},
                    {"name":"refs/tags/release/2.6.0","objectId":"other"}
                ]}))
            }
        })
        .expect(2).mount(&server).await;
    Mock::given(method("POST"))
        .and(path(
            "/org/project/_apis/git/repositories/repo/commitsbatch",
        ))
        .and(body_partial_json(json!({"ids":["other","shared"]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[
            {"commitId":"shared","comment":"Hotfix","committer":{"date":"2026-11-02T08:00:00Z"}},
            {"commitId":"other","comment":"Release","committer":{"date":"2026-11-01T08:00:00Z"}}
        ]})))
        .expect(1)
        .mount(&server)
        .await;
    let tags = api(&server)?.repo_refs("repo", true).await?;
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0]["targetCommit"]["commitId"], "shared");
    assert_eq!(tags[2]["targetCommit"]["comment"], "Hotfix");
    assert_eq!(ref_commit(&tags[2]), "shared");
    Ok(())
}

#[tokio::test]
async fn refs_reject_repeated_tokens_and_empty_refs_need_no_commit_queries() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/org/project/_apis/git/repositories/repo/refs"))
        .and(query_param("filter", "heads/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ms-continuationtoken", "same")
                .set_body_json(json!({"value":[]})),
        )
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/project/_apis/git/repositories/repo/refs"))
        .and(query_param("filter", "tags/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[]})))
        .expect(1)
        .mount(&server)
        .await;
    let api = api(&server)?;
    assert!(api.repo_refs("repo", false).await.is_err());
    assert!(api.repo_refs("repo", true).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn commit_changes_page_and_root_commit_diffs_need_no_parent() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(
            "/org/project/_apis/git/repositories/repo/commits/root",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"commitId":"root","comment":"Initial commit","parents":[]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let page: Vec<_> = (0..100)
        .map(|i| json!({"changeType":"add","item":{"path":format!("/file-{i}"),"isFolder":i == 0}}))
        .collect();
    for (skip, changes) in [
        (0, page),
        (
            100,
            vec![json!({"changeType":"add","item":{"path":"/last"}})],
        ),
    ] {
        Mock::given(method("GET"))
            .and(path(
                "/org/project/_apis/git/repositories/repo/commits/root/changes",
            ))
            .and(query_param("top", "100"))
            .and(query_param("skip", skip.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"changes":changes})))
            .expect(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/org/project/_apis/git/repositories/repo/items"))
        .and(query_param("path", "/file-1"))
        .and(query_param("versionDescriptor.version", "root"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"content":"first\n"})))
        .expect(1)
        .mount(&server)
        .await;
    let api = api(&server)?;
    let value = api.repo_commit_changes("repo", "root").await?;
    assert_eq!(value["detailChanges"]["base"], "");
    assert_eq!(
        value["detailChanges"]["files"]
            .as_array()
            .context("Files")?
            .len(),
        100
    );
    let diff = api.pr_file_diff(&value["detailChanges"], 0).await?;
    assert!(diff.contains("+first"));
    Ok(())
}
