use super::*;
use crate::service::tests::api;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

const COMMIT: &str = "1111111111111111111111111111111111111111";
const ANNOTATED: &str = "2222222222222222222222222222222222222222";

#[test]
fn tag_names_and_targets_are_validated_without_a_local_checkout() -> Result<()> {
    for name in [
        "v1.0.0",
        "release/1.0.0",
        "refs/tags/release/1.0.0",
        "版本/一",
    ] {
        assert!(tag_name(name)?.starts_with("refs/tags/"));
    }
    for name in [
        "",
        "refs/tags/",
        "/v1",
        "v1/",
        "a//b",
        ".hidden",
        "a/.hidden",
        "v1.lock",
        "a.lock/b",
        "a..b",
        "a@{b",
        "a b",
        "a\nb",
        "a\tb",
        "a~b",
        "a^b",
        "a:b",
        "a?b",
        "a*b",
        "a[b",
        "a\\b",
        "v1.",
    ] {
        assert!(tag_name(name).is_err(), "accepted {name:?}");
    }
    for target in [
        "",
        "abc1234",
        ZERO,
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
    ] {
        assert!(
            TagChange::Create {
                name: "v1".into(),
                target: target.into()
            }
            .validate()
            .is_err()
        );
    }
    assert!(
        TagChange::Rename {
            old: "refs/tags/v1".into(),
            name: "v1".into(),
            object: COMMIT.into()
        }
        .validate()
        .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn create_rename_and_delete_use_compare_and_swap_and_preserve_tag_objects() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/org/project/_apis/git/repositories/repo/refs"))
        .respond_with(|request: &wiremock::Request| {
            let Ok(body) = request.body_json::<Value>() else {
                return ResponseTemplate::new(400);
            };
            ResponseTemplate::new(200).set_body_json(json!({"value":[{"name":body[0]["name"],"success":true,"updateStatus":"succeeded"}]}))
        }).expect(4).mount(&server).await;
    let api = api(&server)?;
    for change in [
        TagChange::Create {
            name: "release/v1".into(),
            target: COMMIT.into(),
        },
        TagChange::Rename {
            old: "refs/tags/release/v1".into(),
            name: "refs/tags/release/v2".into(),
            object: ANNOTATED.into(),
        },
        TagChange::Delete {
            name: "refs/tags/release/v2".into(),
            object: ANNOTATED.into(),
        },
    ] {
        api.repo_change_tag("repo", &change).await?;
    }
    let requests = server.received_requests().await.context("Requests")?;
    for (request, (name, old, new)) in requests.iter().zip([
        ("release/v1", ZERO, COMMIT),
        ("release/v2", ZERO, ANNOTATED),
        ("release/v1", ANNOTATED, ZERO),
        ("release/v2", ANNOTATED, ZERO),
    ]) {
        assert_eq!(
            request.body_json::<Value>()?,
            json!([{"name":format!("refs/tags/{name}"),"oldObjectId":old,"newObjectId":new}])
        );
    }
    Ok(())
}

#[tokio::test]
async fn failed_destination_never_deletes_the_original_or_retries() -> Result<()> {
    for status in [
        "staleOldObjectId",
        "createTagPermissionRequired",
        "locked",
        "refNameConflict",
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[{"name":"refs/tags/new","success":false,"updateStatus":status,"customMessage":"Denied"}]})))
            .expect(1).mount(&server).await;
        let error = api(&server)?
            .repo_change_tag(
                "repo",
                &TagChange::Rename {
                    old: "refs/tags/old".into(),
                    name: "refs/tags/new".into(),
                    object: ANNOTATED.into(),
                },
            )
            .await
            .err()
            .context("must reject per-ref failures")?;
        assert!(error.to_string().contains(status));
        let requests = server.received_requests().await.context("Requests")?;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].body_json::<Value>()?[0]["newObjectId"],
            ANNOTATED
        );
    }
    Ok(())
}

#[tokio::test]
async fn rename_reports_partial_success_when_source_deletion_fails() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|request: &wiremock::Request| {
            let Ok(body) = request.body_json::<Value>() else {
                return ResponseTemplate::new(400);
            };
            let create = body[0]["oldObjectId"] == ZERO;
            ResponseTemplate::new(200).set_body_json(json!({"value":[{
                "name":body[0]["name"], "success":create,
                "updateStatus":if create { "succeeded" } else { "staleOldObjectId" }
            }]}))
        })
        .expect(2)
        .mount(&server)
        .await;
    let error = api(&server)?
        .repo_change_tag(
            "repo",
            &TagChange::Rename {
                old: "refs/tags/old".into(),
                name: "refs/tags/new".into(),
                object: ANNOTATED.into(),
            },
        )
        .await
        .err()
        .context("source changed concurrently")?
        .to_string();
    assert!(error.contains("Created tag new, but could not delete old"));
    assert!(error.contains("staleOldObjectId"));
    assert!(error.contains("Both names may exist"));
    Ok(())
}

#[tokio::test]
async fn missing_results_and_http_failures_are_not_reported_as_success() -> Result<()> {
    for response in [
        ResponseTemplate::new(200).set_body_json(json!({"value":[]})),
        ResponseTemplate::new(500),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            api(&server)?
                .repo_change_tag(
                    "repo",
                    &TagChange::Delete {
                        name: "v1".into(),
                        object: COMMIT.into()
                    }
                )
                .await
                .is_err()
        );
        assert_eq!(
            server.received_requests().await.context("Requests")?.len(),
            1
        );
    }
    Ok(())
}
