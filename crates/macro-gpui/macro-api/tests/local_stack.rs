use anyhow::{Context as _, ensure};
use macro_api::{
    ChannelMessageKind, ChannelMessageRequest, MacroApi, MacroApiConfig,
    PatchChannelMessageRequest, UnifiedSearchMatchType, UnifiedSearchOn, UnifiedSearchRequest,
};
use serde_json::Value;
use std::{fs, path::PathBuf};

#[test]
#[ignore = "requires Macro's seeded `cargo x local-e2e` stack"]
fn native_client_round_trips_auth_documents_and_channel_mutations() -> anyhow::Result<()> {
    reqwest_client::runtime().block_on(async {
        let proxy_origin = required_env("LOCAL_E2E_BACKEND_ORIGIN")?;
        let web_origin = required_env("LOCAL_E2E_BASE_URL")?;
        let token = required_env("LOCAL_E2E_TOKEN")?;
        let source_dir = PathBuf::from(required_env("MACRO_SOURCE_DIR")?);
        let fixture: Value = serde_json::from_slice(
            &fs::read(source_dir.join("tooling/seed_cli/seed/local_e2e/manifest.json"))
                .context("read Macro local E2E manifest")?,
        )
        .context("decode Macro local E2E manifest")?;
        let user_email = fixture["user"]["email"]
            .as_str()
            .context("manifest user email")?;
        let document_id = fixture["documents"]["projectRoadmap"]["id"]
            .as_str()
            .context("manifest roadmap id")?;
        let document_name = fixture["documents"]["projectRoadmap"]["name"]
            .as_str()
            .context("manifest roadmap name")?;
        let channel_id = fixture["channels"]["general"]["id"]
            .as_str()
            .context("manifest general channel id")?;
        let seeded_message = fixture["channels"]["general"]["message"]
            .as_str()
            .context("manifest general channel message")?;
        let deep_thread = &fixture["channels"]["general"]["deepThread"];
        let deep_thread_parent_id = deep_thread["parentMessageId"]
            .as_str()
            .context("manifest deep-thread parent message id")?;
        let deep_thread_target_id = deep_thread["targetReplyId"]
            .as_str()
            .context("manifest deep-thread target reply id")?;
        let deep_thread_target_text = deep_thread["targetReplyText"]
            .as_str()
            .context("manifest deep-thread target reply text")?;
        let oversized_reply_id = deep_thread["oversizedReplyId"]
            .as_str()
            .context("manifest oversized reply id")?;
        let oversized_reply_text = deep_thread["oversizedReplyText"]
            .as_str()
            .context("manifest oversized reply text")?;

        let api = MacroApi::new(MacroApiConfig::for_local_proxy(&proxy_origin, &web_origin)?);
        let user = api.me(&token).await.context("native auth contract")?;
        ensure!(
            user.user_id.contains(user_email),
            "authenticated fixture user mismatch: {}",
            user.user_id
        );
        api.set_token(token);

        let channels = api
            .list_channels(Some(100), None)
            .await
            .context("native channel-list contract")?;
        ensure!(
            contains_string(&channels, channel_id),
            "seeded general channel missing from native list response"
        );

        let channel = api
            .get_channel(channel_id)
            .await
            .context("native channel-detail contract")?;
        ensure!(
            contains_string(&channel, seeded_message),
            "seeded message missing from native channel detail"
        );

        for reply_id in [deep_thread_target_id, oversized_reply_id] {
            let resolved = api
                .resolve_channel_message(channel_id, reply_id)
                .await
                .with_context(|| format!("resolve seeded channel reply {reply_id}"))?;
            ensure!(
                resolved.channel_id == channel_id
                    && resolved.message_id == reply_id
                    && resolved.thread_id == deep_thread_parent_id
                    && resolved.kind == ChannelMessageKind::ThreadReply,
                "seeded reply resolved to the wrong native thread target: {resolved:?}"
            );
        }
        let thread_replies = api
            .list_channel_thread_replies(channel_id, deep_thread_parent_id)
            .await
            .context("native deep-thread replies contract")?;
        ensure!(
            contains_string(&thread_replies, deep_thread_target_id)
                && contains_string(&thread_replies, deep_thread_target_text)
                && contains_string(&thread_replies, oversized_reply_id)
                && contains_string(&thread_replies, oversized_reply_text),
            "seeded deep-thread targets missing from native replies response"
        );

        let document = api
            .get_document(document_id)
            .await
            .context("native document-detail contract")?;
        ensure!(
            contains_string(&document, document_id) && contains_string(&document, document_name),
            "seeded roadmap document did not round-trip through the native client"
        );

        let search = api
            .search(
                &UnifiedSearchRequest {
                    query: document_name.to_owned(),
                    match_type: UnifiedSearchMatchType::Partial,
                    search_on: Some(UnifiedSearchOn::NameContent),
                    include_crm: true,
                    collapse: None,
                    filters: None,
                },
                25,
                None,
            )
            .await
            .context("native typed unified-search contract")?;
        ensure!(
            search
                .results
                .iter()
                .any(|result| contains_string(result.raw(), document_id)
                    && contains_string(result.raw(), document_name)),
            "seeded roadmap document missing from native typed search response"
        );

        let notification_id = fixture["navigation"]["unreadNotificationId"]
            .as_str()
            .context("manifest unread notification id")?;
        let notifications = api
            .list_notifications(100, None)
            .await
            .context("native notification-list contract")?;
        ensure!(
            notifications
                .items
                .iter()
                .any(|notification| notification.id == notification_id),
            "seeded unread notification missing from native notification response"
        );

        let nonce = format!(
            "macro-gpui-local-e2e-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        );
        let initial_content = format!("Native GPUI local-stack mutation {nonce}");
        let created = api
            .post_channel_message(
                channel_id,
                &ChannelMessageRequest {
                    attachments: Vec::new(),
                    content: initial_content,
                    mentions: Vec::new(),
                    nonce: Some(nonce.clone()),
                    thread_id: None,
                },
            )
            .await
            .context("native channel send contract")?;

        let mutation_result = async {
            ensure!(
                created.nonce.as_deref() == Some(nonce.as_str()),
                "server did not echo the native mutation nonce"
            );
            let edited_content = format!("Native GPUI local-stack edit {nonce}");
            api.patch_channel_message(
                channel_id,
                &created.id,
                &PatchChannelMessageRequest {
                    attachment_ids_to_delete: None,
                    attachments_to_add: None,
                    content: Some(edited_content.clone()),
                    mentions: None,
                    nonce: Some(nonce.clone()),
                },
            )
            .await
            .context("native channel edit contract")?;
            let messages = api
                .list_channel_messages_window(channel_id, 25, None, None, Some(&created.id))
                .await
                .context("native around-message pagination contract")?;
            ensure!(
                contains_string(&messages, &created.id)
                    && contains_string(&messages, &edited_content),
                "edited native message missing from around-message page"
            );
            anyhow::Ok(())
        }
        .await;

        let cleanup = api
            .delete_channel_message(channel_id, &created.id, Some(&nonce))
            .await
            .context("clean up native local-stack message");
        mutation_result?;
        cleanup?;
        Ok(())
    })
}

fn required_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} must be set by the local-stack harness"))
}

fn contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value == needle,
        Value::Array(values) => values.iter().any(|value| contains_string(value, needle)),
        Value::Object(values) => values.values().any(|value| contains_string(value, needle)),
        _ => false,
    }
}

#[test]
fn recursive_fixture_assertion_checks_nested_service_payloads() {
    let value = serde_json::json!({"items":[{"data":{"id":"target"}}]});
    assert!(contains_string(&value, "target"));
    assert!(!contains_string(&value, "missing"));
}
