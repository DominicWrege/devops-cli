use super::*;
use crate::service::tests::api;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, method, path, query_param},
};

const COMMIT: &str = "1111111111111111111111111111111111111111";
const ANNOTATED: &str = "2222222222222222222222222222222222222222";

#[tokio::test]
async fn cli_create_sends_a_single_ref_update_and_returns_confirmation() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/org/project/_apis/git/repositories/repo/refs"))
        .and(body_json(json!([{
            "name":"refs/tags/release/v1", "oldObjectId":ZERO, "newObjectId":COMMIT
        }])))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[{
            "name":"refs/tags/release/v1", "success":true, "updateStatus":"succeeded"
        }]})))
        .expect(1)
        .mount(&server)
        .await;
    let message = api(&server)?
        .tag_action(&TagCommand::Create {
            name: "refs/tags/release/v1".into(),
            repo: "repo".into(),
            target: COMMIT.into(),
        })
        .await?;
    assert_eq!(message, "Created tag release/v1");
    assert_eq!(
        server.received_requests().await.context("Requests")?.len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn cli_delete_pages_for_the_exact_tag_and_uses_the_unpeeled_object() -> Result<()> {
    for object in [COMMIT, ANNOTATED] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/org/project/_apis/git/repositories/repo/refs"))
            .and(query_param("filter", "tags/release/v1"))
            .and(query_param("peelTags", "false"))
            .respond_with(move |request: &wiremock::Request| {
                let next = request
                    .url
                    .query_pairs()
                    .any(|(key, value)| key == "continuationToken" && value == "next page");
                if next {
                    ResponseTemplate::new(200).set_body_json(json!({"value":[{
                        "name":"refs/tags/release/v1", "objectId":object, "peeledObjectId":COMMIT
                    }]}))
                } else {
                    ResponseTemplate::new(200)
                        .insert_header("x-ms-continuationtoken", "next page")
                        .set_body_json(json!({"value":[{
                            "name":"refs/tags/release/v10", "objectId":COMMIT
                        }]}))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/org/project/_apis/git/repositories/repo/refs"))
            .and(body_json(json!([{
                "name":"refs/tags/release/v1", "oldObjectId":object, "newObjectId":ZERO
            }])))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[{
                "name":"refs/tags/release/v1", "success":true, "updateStatus":"succeeded"
            }]})))
            .expect(1)
            .mount(&server)
            .await;
        let message = api(&server)?
            .tag_action(&TagCommand::Delete {
                name: "release/v1".into(),
                repo: "repo".into(),
                yes: true,
            })
            .await?;
        assert_eq!(message, "Deleted tag release/v1");
        assert_eq!(
            server.received_requests().await.context("Requests")?.len(),
            3
        );
    }
    Ok(())
}

#[tokio::test]
async fn cli_delete_never_writes_for_a_missing_tag_or_invalid_object() -> Result<()> {
    for tag in [
        json!({"name":"refs/tags/v10", "objectId":COMMIT}),
        json!({"name":"refs/tags/v1", "objectId":ZERO}),
        json!({"name":"refs/tags/v1"}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(query_param("filter", "tags/v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value":[tag]})))
            .expect(1)
            .mount(&server)
            .await;
        let error = api(&server)?
            .tag_action(&TagCommand::Delete {
                name: "v1".into(),
                repo: "repo".into(),
                yes: true,
            })
            .await
            .err()
            .context("Must reject missing tag/object")?;
        assert!(
            error
                .to_string()
                .contains(if tag["name"] == "refs/tags/v10" {
                    "Tag v1 not found"
                } else {
                    "40-character"
                })
        );
        assert_eq!(
            server.received_requests().await.context("Requests")?.len(),
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn cli_tag_validation_prevents_all_requests() -> Result<()> {
    let server = MockServer::start().await;
    let api = api(&server)?;
    for command in [
        TagCommand::Create {
            name: "v1".into(),
            repo: "repo".into(),
            target: "short".into(),
        },
        TagCommand::Delete {
            name: "v1".into(),
            repo: "repo".into(),
            yes: false,
        },
        TagCommand::Delete {
            name: "bad..tag".into(),
            repo: "repo".into(),
            yes: true,
        },
    ] {
        let error = api
            .tag_action(&command)
            .await
            .err()
            .context("Invalid command")?;
        assert_eq!(
            error.downcast_ref::<error::Failure>().map(|e| e.code),
            Some(2)
        );
    }
    assert!(
        server
            .received_requests()
            .await
            .context("Requests")?
            .is_empty()
    );
    Ok(())
}

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
