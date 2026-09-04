use anyhow::{Context, Result, ensure};
use async_tungstenite::{
    WebSocketStream,
    tokio::{ConnectStream, connect_async_with_tls_connector},
    tungstenite::{Message, client::IntoClientRequest, http::header::SEC_WEBSOCKET_PROTOCOL},
};
use base64::Engine as _;
use futures::StreamExt;
use reqwest::{Client, RequestBuilder, multipart};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// A typed non-success response from a Macro service.
///
/// Keeping the status and machine-readable code available lets native product
/// surfaces distinguish authentication, entitlement, conflict, rate-limit,
/// and retryable failures without parsing a display string.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("HTTP {status}: {message}")]
pub struct MacroHttpError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub retry_after: Option<String>,
    /// Structured service error payload when the response body was JSON.
    /// Native flows use this for typed conflicts (for example shared inbox
    /// ownership) without scraping display text.
    pub details: Option<serde_json::Value>,
}

impl MacroHttpError {
    pub fn is_payment_required(&self) -> bool {
        self.status == 402
            || self.status == 403
            || self.code.as_deref().is_some_and(|code| {
                matches!(
                    code.to_ascii_lowercase().as_str(),
                    "payment_required" | "forbidden" | "entitlement_required"
                )
            })
    }

    pub fn is_authentication_required(&self) -> bool {
        matches!(self.status, 401 | 419)
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self.status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
    }

    pub fn shared_inbox_conflict(&self) -> Option<SharedInboxConflictResponse> {
        (self.status == 409)
            .then(|| self.details.clone())
            .flatten()
            .and_then(|details| serde_json::from_value(details).ok())
    }
}

#[derive(Debug)]
struct HttpTextResponse {
    status: u16,
    text: String,
    request_id: Option<String>,
    retry_after: Option<String>,
}

fn macro_http_error(response: HttpTextResponse) -> MacroHttpError {
    let value = serde_json::from_str::<serde_json::Value>(&response.text).ok();
    let code = value.as_ref().and_then(|value| {
        ["code", "error_code", "errorCode"]
            .into_iter()
            .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
            .map(ToOwned::to_owned)
    });
    let message = value
        .as_ref()
        .and_then(|value| {
            ["message", "error", "detail", "reason"]
                .into_iter()
                .find_map(|key| value.get(key).and_then(serde_json::Value::as_str))
        })
        .map(ToOwned::to_owned)
        .or_else(|| {
            let trimmed = response.text.trim();
            (!trimmed.is_empty()).then(|| trimmed.chars().take(4_096).collect())
        })
        .unwrap_or_else(|| "request failed".into());
    MacroHttpError {
        status: response.status,
        code,
        message,
        request_id: response.request_id,
        retry_after: response.retry_after,
        details: value,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroEnvironment {
    Dev,
    Prod,
    Local,
}

#[derive(Debug, Clone)]
pub struct MacroApiConfig {
    pub auth_base_url: String,
    pub storage_base_url: String,
    pub email_base_url: String,
    pub connection_base_url: String,
    pub cognition_base_url: String,
    pub notification_base_url: String,
    pub contacts_base_url: String,
    pub agent_base_url: String,
    pub search_base_url: String,
    pub static_files_base_url: String,
    pub scheduled_action_base_url: String,
    pub unfurl_base_url: String,
    pub sync_http_base_url: String,
    pub sync_websocket_base_url: String,
    pub web_app_origin: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticFileUpload {
    pub id: String,
    pub file_location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StaticFilePresignResponse {
    id: String,
    file_location: String,
    upload_url: String,
}

impl Default for MacroApiConfig {
    fn default() -> Self {
        Self::for_environment(MacroEnvironment::Local)
    }
}

impl MacroApiConfig {
    /// Hosts mirrored from `macro/packages/sdk/src/config.ts`.
    pub fn for_environment(environment: MacroEnvironment) -> Self {
        let suffix = match environment {
            MacroEnvironment::Dev => "-dev",
            MacroEnvironment::Prod => "",
            MacroEnvironment::Local => {
                return Self {
                    auth_base_url: "http://localhost:8080".into(),
                    storage_base_url: "http://localhost:8086".into(),
                    email_base_url: "http://localhost:8087".into(),
                    connection_base_url: "http://localhost:8082".into(),
                    cognition_base_url: "http://localhost:8085".into(),
                    notification_base_url: "http://localhost:8089".into(),
                    contacts_base_url: "http://localhost:8083".into(),
                    agent_base_url: "http://localhost:8101".into(),
                    search_base_url: "http://localhost:8086".into(),
                    static_files_base_url: "http://localhost:8100".into(),
                    scheduled_action_base_url: "http://localhost:8098".into(),
                    unfurl_base_url: "http://localhost:8095".into(),
                    sync_http_base_url: "http://localhost:8787".into(),
                    sync_websocket_base_url: "ws://localhost:8787".into(),
                    web_app_origin: "http://localhost:3000".into(),
                };
            }
        };

        Self {
            auth_base_url: format!("https://auth-service{suffix}.macro.com"),
            storage_base_url: format!("https://cloud-storage{suffix}.macro.com"),
            email_base_url: format!("https://email-service{suffix}.macro.com"),
            connection_base_url: format!("https://connection-gateway{suffix}.macro.com"),
            cognition_base_url: format!("https://document-cognition{suffix}.macro.com"),
            notification_base_url: format!("https://notifications{suffix}.macro.com"),
            contacts_base_url: format!("https://contacts{suffix}.macro.com"),
            agent_base_url: format!("https://agent-harness{suffix}.macro.com"),
            search_base_url: format!("https://cloud-storage{suffix}.macro.com"),
            static_files_base_url: format!("https://static-file-service{suffix}.macro.com"),
            scheduled_action_base_url: format!("https://agent-schedule{suffix}.macro.com"),
            unfurl_base_url: format!("https://unfurl-service{suffix}.macro.com"),
            sync_http_base_url: match environment {
                MacroEnvironment::Dev => "https://sync-service-dev3.macroverse.workers.dev".into(),
                MacroEnvironment::Prod => {
                    "https://sync-service-prod2.macroverse.workers.dev".into()
                }
                MacroEnvironment::Local => unreachable!(),
            },
            sync_websocket_base_url: match environment {
                MacroEnvironment::Dev => "wss://sync-service-dev3.macroverse.workers.dev".into(),
                MacroEnvironment::Prod => "wss://sync-service-prod2.macroverse.workers.dev".into(),
                MacroEnvironment::Local => unreachable!(),
            },
            web_app_origin: match environment {
                MacroEnvironment::Dev => "https://dev.macro.com".into(),
                MacroEnvironment::Prod => "https://macro.com".into(),
                MacroEnvironment::Local => unreachable!(),
            },
        }
    }

    /// Builds a local configuration through Macro's isolated-stack reverse
    /// proxy. This is the same prefix map generated by Macro's `cargo x`
    /// orchestrator and lets the native client participate in the exact fixture
    /// stack used by the SolidJS and backend E2E suites.
    pub fn for_local_proxy(proxy_origin: &str, web_app_origin: &str) -> Result<Self> {
        let proxy = url::Url::parse(proxy_origin).context("invalid local Macro proxy origin")?;
        let web = url::Url::parse(web_app_origin).context("invalid local Macro web origin")?;
        for (label, url) in [("proxy", &proxy), ("web", &web)] {
            ensure!(
                url.scheme() == "http"
                    && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1")),
                "{label} origin must be a local plaintext HTTP URL"
            );
        }
        ensure!(
            proxy.path() == "/" && proxy.query().is_none() && proxy.fragment().is_none(),
            "proxy origin must not contain a path, query, or fragment"
        );
        let origin = proxy_origin.trim_end_matches('/');
        let websocket_scheme = if proxy.scheme() == "https" {
            "wss"
        } else {
            "ws"
        };
        let websocket_origin = format!(
            "{websocket_scheme}://{}{}",
            proxy
                .host_str()
                .context("local Macro proxy must have a host")?,
            proxy
                .port()
                .map(|port| format!(":{port}"))
                .unwrap_or_default()
        );
        Ok(Self {
            auth_base_url: format!("{origin}/auth"),
            storage_base_url: format!("{origin}/dss"),
            email_base_url: format!("{origin}/email"),
            connection_base_url: format!("{origin}/connection-gateway"),
            cognition_base_url: format!("{origin}/cognition"),
            notification_base_url: format!("{origin}/notification"),
            contacts_base_url: format!("{origin}/contacts"),
            agent_base_url: format!("{origin}/agent-harness"),
            search_base_url: format!("{origin}/dss"),
            static_files_base_url: format!("{origin}/static-file"),
            // The local stack does not currently proxy the scheduled-action
            // worker as a public service. Keep its conventional direct port so
            // tests surface that absence instead of silently routing elsewhere.
            scheduled_action_base_url: "http://localhost:8098".into(),
            unfurl_base_url: format!("{origin}/unfurl"),
            sync_http_base_url: format!("{origin}/sync"),
            sync_websocket_base_url: format!("{websocket_origin}/sync"),
            web_app_origin: web_app_origin.trim_end_matches('/').into(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordlessStartedResponse {
    pub code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SsoRequiredResponse {
    pub idp_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PasswordlessLoginResponse {
    SsoRequired(SsoRequiredResponse),
    Started(PasswordlessStartedResponse),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTokensResponse {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroApiTokenResponse {
    pub macro_api_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub user_id: String,
    pub permissions: Vec<String>,
    pub organization_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentPermissionsTokenResponse {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncPeerMetadata {
    pub peer_id: u64,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncDocumentMetadata {
    #[serde(default)]
    pub peers: Vec<SyncPeerMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncNodeBlame {
    pub peer_id: String,
    pub user_id: Option<String>,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagSetResponse {
    pub scope: String,
    pub definition: Option<TagDefinition>,
    #[serde(default)]
    pub options: Vec<TagOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagDefinition {
    pub id: String,
    #[serde(alias = "displayName")]
    pub display_name: String,
    #[serde(alias = "dataType")]
    pub data_type: String,
    #[serde(default, alias = "isMultiSelect")]
    pub is_multi_select: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum PropertyOwner {
    User { user_id: String },
    Team { team_id: String },
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyDefinition {
    pub created_at: String,
    pub data_type: String,
    pub display_name: String,
    pub id: String,
    pub is_metadata: bool,
    pub is_multi_select: bool,
    pub is_system: bool,
    pub owner: PropertyOwner,
    #[serde(default)]
    pub specific_entity_type: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PropertyDefinitionResponse {
    WithOptions {
        definition: PropertyDefinition,
        #[serde(default)]
        property_options: Vec<TagOption>,
    },
    Definition(PropertyDefinition),
}

impl PropertyDefinitionResponse {
    pub fn definition(&self) -> &PropertyDefinition {
        match self {
            Self::WithOptions { definition, .. } | Self::Definition(definition) => definition,
        }
    }

    pub fn options(&self) -> &[TagOption] {
        match self {
            Self::WithOptions {
                property_options, ..
            } => property_options,
            Self::Definition(_) => &[],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CreatePropertyDataType {
    Boolean,
    Date,
    Number,
    String,
    SelectNumber {
        multi: bool,
        options: Vec<SelectNumberOption>,
    },
    SelectString {
        multi: bool,
        options: Vec<SelectStringOption>,
    },
    Entity {
        multi: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        specific_type: Option<String>,
    },
    Link {
        multi: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectNumberOption {
    pub display_order: i64,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectStringOption {
    pub display_order: i64,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreatePropertyDefinitionRequest {
    pub data_type: CreatePropertyDataType,
    pub display_name: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagOption {
    pub id: String,
    #[serde(alias = "propertyDefinitionId")]
    pub property_definition_id: String,
    #[serde(alias = "displayOrder")]
    pub display_order: i64,
    pub value: serde_json::Value,
    pub color: Option<String>,
}

impl TagOption {
    pub fn label(&self) -> String {
        self.value
            .get("value")
            .and_then(serde_json::Value::as_str)
            .or_else(|| self.value.as_str())
            .unwrap_or("Unnamed")
            .to_owned()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnsureTagSetRequest {
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddStringPropertyOptionRequest {
    #[serde(rename = "type")]
    pub option_type: String,
    pub option: AddStringPropertyOption,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddStringPropertyOption {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub display_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UpdatePropertyOptionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentComment {
    pub comment_id: i64,
    pub created_at: Option<String>,
    pub deleted_at: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub order: Option<i64>,
    pub owner: String,
    pub sender: Option<String>,
    pub text: String,
    pub thread_id: i64,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCommentThreadInfo {
    pub created_at: Option<String>,
    pub deleted_at: Option<String>,
    pub document_id: String,
    pub metadata: Option<serde_json::Value>,
    pub owner: String,
    pub resolved: bool,
    pub thread_id: i64,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentCommentThread {
    pub comments: Vec<DocumentComment>,
    pub thread: DocumentCommentThreadInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentCommentsResponse {
    pub data: Vec<DocumentCommentThread>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateDocumentCommentRequest {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<DocumentCommentMentions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EditDocumentCommentRequest {
    pub thread_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<DocumentCommentMentions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCommentMentions {
    pub mention_id: String,
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDocumentCommentRequest {
    pub remove_anchor_thread_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDocumentCommentThreadInfo {
    pub thread_id: i64,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteDocumentCommentResponse {
    pub document_id: String,
    pub comment_id: i64,
    pub thread: DeleteDocumentCommentThreadInfo,
    #[serde(default)]
    pub anchor: Option<serde_json::Value>,
}

/// Wire request for `/channels/{channel_id}/message`.
///
/// Keep this in lockstep with Macro's generated `PostMessageRequest`; the
/// plural `/messages` endpoint is a query endpoint and must not be used to
/// create messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMessageRequest {
    pub attachments: Vec<NewChannelAttachment>,
    pub content: String,
    pub mentions: Vec<SimpleMention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NewChannelAttachment {
    pub entity_id: String,
    pub entity_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "reference_type")]
pub enum AttachmentEntityReference {
    #[serde(rename = "channel")]
    Channel {
        attachment_created_at: String,
        channel_id: String,
        #[serde(default)]
        channel_name: Option<String>,
        message_content: String,
        message_created_at: String,
        message_id: String,
        sender_id: String,
        #[serde(default)]
        thread_id: Option<String>,
    },
    #[serde(rename = "generic")]
    Generic {
        created_at: String,
        entity_id: String,
        entity_type: String,
        source_entity_id: String,
        source_entity_type: String,
        #[serde(default)]
        user_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetAttachmentReferencesResponse {
    #[serde(default)]
    pub references: Vec<AttachmentEntityReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SimpleMention {
    pub entity_id: String,
    pub entity_type: String,
}

/// Generated-storage compatible request for `POST /channels/mentions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateEntityMentionRequest {
    pub entity_id: String,
    pub entity_type: String,
    pub source_entity_id: String,
    pub source_entity_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateEntityMentionResponse {
    pub created_at: String,
    pub entity_id: String,
    pub entity_type: String,
    pub id: String,
    pub source_entity_id: String,
    pub source_entity_type: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeleteEntityMentionResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShareAccessLevel {
    View,
    Comment,
    Edit,
    Owner,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntityMemberRole {
    Owner,
    Admin,
    Member,
}

impl EntityMemberRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
}

/// Macro storage's generated discriminated permission union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApiEntityPermission {
    AccessLevel { access_level: ShareAccessLevel },
    ChannelViewOnly,
    ChannelRole { role: EntityMemberRole },
    TeamRole { role: EntityMemberRole },
}

/// Macro storage's generated permission response envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GetEntityPermissionResponse {
    Access { permission: ApiEntityPermission },
    NoAccess,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum LinkShare {
    Public,
    Team,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelSharePermission {
    pub access_level: ShareAccessLevel,
    pub channel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SharePermissionV2 {
    #[serde(default)]
    pub channel_share_permissions: Option<Vec<ChannelSharePermission>>,
    pub id: String,
    #[serde(default)]
    pub link_share: Option<LinkShare>,
    #[serde(default)]
    pub link_share_access_level: Option<ShareAccessLevel>,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GetDocumentPermissionsResponseV2 {
    pub document_permissions: SharePermissionV2,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShareUpdateOperation {
    Add,
    Remove,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannelSharePermission {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_level: Option<ShareAccessLevel>,
    pub channel_id: String,
    pub operation: ShareUpdateOperation,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSharePermissionRequestV2 {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_share_permissions: Option<Vec<UpdateChannelSharePermission>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_share: Option<Option<LinkShare>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_share_access_level: Option<Option<ShareAccessLevel>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditDocumentShareRequest<'a> {
    share_permission: &'a UpdateSharePermissionRequestV2,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditProjectShareRequest<'a> {
    share_permission: &'a UpdateSharePermissionRequestV2,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditThreadShareRequest<'a> {
    share_permission: &'a UpdateSharePermissionRequestV2,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelMessageCreated {
    pub id: String,
    #[serde(default)]
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChannelMessageKind {
    TopLevelMessage,
    ThreadReply,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedChannelMessage {
    pub channel_id: String,
    pub created_at: String,
    pub kind: ChannelMessageKind,
    pub message_id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelActivityType {
    View,
    Interact,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelActivity {
    pub id: String,
    pub user_id: String,
    pub channel_id: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub viewed_at: Option<String>,
    #[serde(default)]
    pub interacted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordChannelActivityRequest {
    pub channel_id: String,
    pub activity_type: ChannelActivityType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PatchChannelMessageRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_ids_to_delete: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments_to_add: Option<Vec<NewChannelAttachment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mentions: Option<Vec<SimpleMention>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReactionAction {
    Add,
    Remove,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelReactionRequest {
    pub action: ReactionAction,
    pub emoji: String,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TypingAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelTypingRequest {
    pub action: TypingAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Public,
    Private,
    DirectMessage,
    Team,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChannelRequest {
    #[serde(default)]
    pub auto_join_team: bool,
    pub channel_type: ChannelType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub participants: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelCreated {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetOrCreateDmRequest {
    pub recipient_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetOrCreatePrivateRequest {
    pub recipients: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GetOrCreateChannelAction {
    Get,
    Create,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetOrCreateChannelResponse {
    pub action: GetOrCreateChannelAction,
    pub channel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelJoinCodeResponse {
    pub join_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchChannelRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_join_team: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convert_to_team_channel: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChannelParticipant {
    pub channel_id: String,
    pub joined_at: String,
    pub role: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChannelParticipantsRequest {
    participants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ReminderSchedule {
    Once {
        #[serde(rename = "remindAt")]
        remind_at: String,
    },
    Recurring {
        cron: String,
        timezone: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    #[serde(default)]
    pub completed_at: Option<String>,
    pub created_at: String,
    pub description: String,
    pub enabled: bool,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub entity_type: Option<String>,
    pub id: String,
    pub next_run_at: String,
    pub schedule: ReminderSchedule,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderPage {
    #[serde(default)]
    pub next_cursor: Option<String>,
    pub reminders: Vec<Reminder>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateReminderRequest {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity_type: Option<String>,
    pub schedule: ReminderSchedule,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReminderRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<ReminderSchedule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MacroNotification {
    pub entity_id: String,
    pub entity_type: String,
    pub created_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
    pub done: bool,
    pub id: String,
    pub notification_event_type: String,
    pub notification_metadata: serde_json::Value,
    pub owner_id: String,
    #[serde(default)]
    pub sender_id: Option<String>,
    pub sent: bool,
    pub updated_at: String,
    #[serde(default)]
    pub viewed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NotificationPage {
    pub items: Vec<MacroNotification>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct NotificationBulkRequest {
    notification_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct NotificationTypePreferences {
    pub disabled_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationUnsubscribe {
    pub item_id: String,
    pub item_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationEntityInput {
    pub entity_id: String,
    pub entity_type: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum NotificationUpdateOperation {
    MarkDone,
    MarkSeen,
    MarkUndone,
}

#[derive(Debug, Deserialize)]
struct GraphqlNotificationMutationResponse {
    data: Option<GraphqlNotificationMutationData>,
    #[serde(default)]
    errors: Vec<GraphqlError>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphqlNotificationMutationData {
    #[serde(default)]
    update_notifications_for_entity: Vec<GraphqlNotificationId>,
}

#[derive(Debug, Deserialize)]
struct GraphqlNotificationId {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GraphqlError {
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskRequest<'a> {
    pub task_name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub property_values: Option<&'a [TaskPropertyInput]>,
    pub share_with_team: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTaskResponse {
    pub document_id: String,
    pub document_metadata: serde_json::Value,
    #[serde(default)]
    pub initial_snapshot: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskPropertyInput {
    pub property_id: String,
    pub value: SetPropertyValue,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyEntityReference {
    pub entity_id: String,
    pub entity_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub specific_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PropertyTargetReference {
    pub entity_id: String,
    pub entity_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkUpdateEntitiesPropertyOptionsRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_option_ids: Vec<String>,
    pub entities: Vec<PropertyTargetReference>,
    pub property_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_option_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkEntityOptionUpdateResult {
    pub entity_id: String,
    pub entity_type: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub option_ids: Option<Vec<String>>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BulkUpdateEntitiesPropertyOptionsResponse {
    pub results: Vec<BulkEntityOptionUpdateResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SetPropertyValue {
    Boolean {
        value: bool,
    },
    Date {
        value: String,
    },
    Number {
        value: f64,
    },
    String {
        value: String,
    },
    SelectOption {
        option_id: String,
    },
    MultiSelectOption {
        option_ids: Vec<String>,
    },
    EntityReference {
        reference: PropertyEntityReference,
    },
    MultiEntityReference {
        references: Vec<PropertyEntityReference>,
    },
    Link {
        url: String,
    },
    MultiLink {
        urls: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateSnippetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub snippet_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateSnippetResponse {
    pub document_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMarkdownResponse {
    pub document_id: String,
    #[serde(default)]
    pub document_metadata: serde_json::Value,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CopyDocumentRequest {
    #[serde(rename = "documentName")]
    pub document_name: String,
    #[serde(rename = "versionId", skip_serializing_if = "Option::is_none")]
    pub version_id: Option<SyncServiceVersionId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SyncServiceVersionId {
    pub counter: i32,
    pub peer: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CopiedDocument {
    pub document_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CopyDocumentResponseEnvelope {
    data: CopyDocumentResponseData,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyDocumentResponseData {
    document_metadata: CopiedDocument,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditDocumentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_name: Option<String>,
    /// `None` omits the field; `Some(None)` moves the document to the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetDocumentTeamShareRequest {
    #[serde(rename = "shareWithTeam")]
    pub share_with_team: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateBinaryDocumentRequest {
    pub document_name: String,
    pub sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadedDocument {
    pub document_id: String,
    pub document_name: String,
    #[serde(default)]
    pub document_version_id: Option<u64>,
    #[serde(default)]
    pub file_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadProgress {
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct UploadControl {
    cancelled: Arc<AtomicBool>,
}

impl UploadControl {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

fn bounded_file_digest(
    path: &Path,
    maximum_bytes: u64,
    control: &UploadControl,
) -> Result<(String, String, u64)> {
    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to inspect upload source {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "upload source {} is not a regular file",
        path.display()
    );
    ensure!(
        metadata.len() <= maximum_bytes,
        "upload source {} exceeds the {maximum_bytes} byte limit",
        path.display()
    );
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to open upload source {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut total_bytes = 0_u64;
    loop {
        ensure!(!control.is_cancelled(), "upload cancelled");
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("failed to read upload source {}", path.display()))?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes
            .checked_add(read as u64)
            .context("upload source size overflow")?;
        ensure!(
            total_bytes <= maximum_bytes,
            "upload source {} grew beyond the {maximum_bytes} byte limit",
            path.display()
        );
        digest.update(&buffer[..read]);
    }
    let digest = digest.finalize();
    Ok((
        format!("{digest:x}"),
        base64::engine::general_purpose::STANDARD.encode(digest),
        total_bytes,
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadExtractFolderRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadExtractFolderResponseData {
    pub presigned_url: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadExtractFolderResponse {
    pub data: UploadExtractFolderResponseData,
    pub error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingFolderUpload {
    pub name: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DocumentContentState {
    #[default]
    Unknown,
    Pending,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentContentLocation {
    pub state: DocumentContentState,
    #[serde(default)]
    pub location: Option<String>,
}

/// The lifecycle fields common to every generated `LocationResponseV3`
/// variant. The URL/BOM payload differs by `type`, but upload finalization only
/// depends on `content`, so the remaining generated fields are deliberately
/// retained as loose JSON instead of guessing a narrower union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DocumentLocationResponse {
    pub content: DocumentContentLocation,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(rename = "type", default)]
    pub location_type: String,
    #[serde(default)]
    pub presigned_url: Option<String>,
    #[serde(default)]
    pub presigned_urls: Vec<serde_json::Value>,
}

impl DocumentLocationResponse {
    pub fn content_is_ready(&self) -> bool {
        self.content.state == DocumentContentState::Ready
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskDuplicate {
    pub id: String,
    pub task_id: String,
    pub task_name: String,
    pub vector_score: f64,
    #[serde(default)]
    pub judge_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskDuplicatesResponse {
    #[serde(default)]
    pub duplicates: Vec<TaskDuplicate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskSimilarityResult {
    pub task_id: String,
    pub task_name: String,
    pub vector_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSimilaritySearchResponse {
    #[serde(default)]
    pub results: Vec<TaskSimilarityResult>,
}

/// Exact top-level request contract generated by Macro's search service.
///
/// Search filters remain entity-specific instead of being passed through as a
/// free-form JSON object. This makes SDK drift fail at the Rust boundary while
/// still allowing the response union to preserve fields added by newer Macro
/// services.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchRequest {
    pub query: String,
    pub match_type: UnifiedSearchMatchType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_on: Option<UnifiedSearchOn>,
    #[serde(default)]
    pub include_crm: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collapse: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<UnifiedSearchEntityFilters>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UnifiedSearchMatchType {
    Exact,
    Partial,
    Regexp,
    Query,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UnifiedSearchOn {
    Name,
    Content,
    NameContent,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchEntityFilters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_event_filters: Option<UnifiedSearchCalendarEventFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub call_filters: Option<UnifiedSearchCallFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_filters: Option<UnifiedSearchChannelFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_thread_filters: Option<UnifiedSearchChannelThreadFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_filters: Option<UnifiedSearchChatFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crm_company_filters: Option<UnifiedSearchCrmCompanyFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_filters: Option<UnifiedSearchDocumentFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email_filters: Option<UnifiedSearchEmailFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreign_entity_filters: Option<UnifiedSearchForeignEntityFilters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_filters: Option<UnifiedSearchProjectFilters>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tag_option_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_filter_mode: Option<UnifiedSearchTagFilterMode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UnifiedSearchTagFilterMode {
    Any,
    All,
}

macro_rules! search_id_filter {
    ($name:ident, $field:ident) => {
        #[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
        pub struct $name {
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub $field: Vec<String>,
        }
    };
}

search_id_filter!(UnifiedSearchCalendarEventFilters, calendar_event_ids);
search_id_filter!(UnifiedSearchCrmCompanyFilters, company_ids);
search_id_filter!(UnifiedSearchForeignEntityFilters, ids);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchCallFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub call_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchChannelFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channel_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sender_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mentions: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchChannelThreadFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thread_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_sender_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub participant_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchChatFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chat_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchDocumentFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub document_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_types: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchEmailFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email_thread_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub senders: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnifiedSearchProjectFilters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedSearchResponseItem {
    kind: UnifiedSearchResultKind,
    raw: serde_json::Value,
}

impl UnifiedSearchResponseItem {
    pub fn kind(&self) -> &UnifiedSearchResultKind {
        &self.kind
    }

    pub fn raw(&self) -> &serde_json::Value {
        &self.raw
    }

    pub fn into_raw(self) -> serde_json::Value {
        self.raw
    }
}

impl Serialize for UnifiedSearchResponseItem {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.raw.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for UnifiedSearchResponseItem {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = serde_json::Value::deserialize(deserializer)?;
        let object = raw
            .as_object()
            .ok_or_else(|| serde::de::Error::custom("unified search result must be an object"))?;
        let kind = object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| serde::de::Error::missing_field("type"))?;
        Ok(Self {
            kind: UnifiedSearchResultKind::from_wire(kind),
            raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifiedSearchResultKind {
    Document,
    Chat,
    Email,
    ChannelMessage,
    Channel,
    Project,
    Call,
    Company,
    CalendarEvent,
    Unknown(String),
}

impl UnifiedSearchResultKind {
    fn from_wire(value: &str) -> Self {
        match value {
            "document" => Self::Document,
            "chat" => Self::Chat,
            "email" => Self::Email,
            "channelMessage" => Self::ChannelMessage,
            "channel" => Self::Channel,
            "project" => Self::Project,
            "call" => Self::Call,
            "company" => Self::Company,
            "calendarEvent" => Self::CalendarEvent,
            unknown => Self::Unknown(unknown.to_owned()),
        }
    }

    pub fn as_wire(&self) -> &str {
        match self {
            Self::Document => "document",
            Self::Chat => "chat",
            Self::Email => "email",
            Self::ChannelMessage => "channelMessage",
            Self::Channel => "channel",
            Self::Project => "project",
            Self::Call => "call",
            Self::Company => "company",
            Self::CalendarEvent => "calendarEvent",
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnifiedSearchResponse {
    #[serde(default)]
    pub results: Vec<UnifiedSearchResponseItem>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DismissTaskDuplicatesRequest {
    pub match_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailContactInput {
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub photo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmailDraftInput {
    pub subject: String,
    pub to: Vec<EmailContactInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<EmailContactInput>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bcc: Vec<EmailContactInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_db_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replying_to_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_macro: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub db_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers_json: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_signature: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub message: EmailDraftInput,
}

/// The email service's generated `SendMessageResponse` contract.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SendEmailMessageResponse {
    pub message: EmailDraftOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEmailDraftRequest {
    pub draft: EmailDraftInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmailDraftOutput {
    #[serde(default)]
    pub bcc: Vec<EmailContactInput>,
    pub body_html: Option<String>,
    pub body_macro: Option<String>,
    pub body_text: Option<String>,
    #[serde(default)]
    pub cc: Vec<EmailContactInput>,
    pub db_id: Option<String>,
    pub headers_json: Option<serde_json::Value>,
    pub link_id: String,
    pub provider_id: Option<String>,
    pub provider_thread_id: Option<String>,
    pub replying_to_id: Option<String>,
    pub send_time: Option<String>,
    pub subject: String,
    pub thread_db_id: Option<String>,
    #[serde(default)]
    pub to: Vec<EmailContactInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateEmailDraftResponse {
    pub draft: EmailDraftOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleEmailDraftRequest {
    pub send_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleEmailDraftResponse {
    pub message_id: String,
    pub send_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledEmailMessage {
    pub db_id: String,
    pub thread_db_id: String,
    pub link_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub body_text: Option<String>,
    #[serde(default)]
    pub body_html_sanitized: Option<String>,
    #[serde(default)]
    pub body_macro: Option<String>,
    #[serde(default)]
    pub to: Vec<EmailContactInput>,
    #[serde(default)]
    pub cc: Vec<EmailContactInput>,
    #[serde(default)]
    pub bcc: Vec<EmailContactInput>,
    #[serde(default)]
    pub replying_to_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_thread_id: Option<String>,
    #[serde(default)]
    pub scheduled_send_time: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledEmailDraftsResponse {
    #[serde(default)]
    pub messages: Vec<ScheduledEmailMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddEmailDraftAttachmentRequest {
    pub file_name: String,
    pub sha: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddEmailDraftAttachmentResponse {
    pub attachment_id: String,
    pub content_type: String,
    pub upload_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddForwardedEmailAttachmentRequest {
    pub attachment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddForwardedEmailAttachmentResponse {
    pub attachment_id: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailAttachment {
    #[serde(default)]
    pub content_id: Option<String>,
    #[serde(default)]
    pub data_url: Option<String>,
    pub db_id: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub sfs_id: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EmailAccessLevel {
    View,
    Comment,
    Edit,
    Owner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailContactInfo {
    pub email: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub photo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailDraftAttachment {
    pub content_type: String,
    pub draft_id: String,
    pub file_name: String,
    pub id: String,
    pub s3_key: String,
    pub sha: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForwardedEmailAttachment {
    pub attachment_id: String,
    pub draft_id: String,
    #[serde(default)]
    pub filename: Option<String>,
    pub message_provider_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub provider_attachment_id: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailMessageLabel {
    pub created_at: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub label_list_visibility: Option<String>,
    pub link_id: String,
    #[serde(default)]
    pub message_list_visibility: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    pub provider_label_id: String,
    #[serde(default)]
    pub type_: Option<String>,
}

/// The generated email service representation of one fully assembled message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmailMessageDetail {
    #[serde(default)]
    pub attachments: Vec<EmailAttachment>,
    #[serde(default)]
    pub attachments_draft: Vec<EmailDraftAttachment>,
    #[serde(default)]
    pub attachments_forwarded: Vec<ForwardedEmailAttachment>,
    #[serde(default)]
    pub bcc: Vec<EmailContactInfo>,
    #[serde(default)]
    pub body_html_sanitized: Option<String>,
    #[serde(default)]
    pub body_macro: Option<String>,
    #[serde(default)]
    pub body_replyless: Option<String>,
    #[serde(default)]
    pub body_text: Option<String>,
    #[serde(default)]
    pub cc: Vec<EmailContactInfo>,
    pub created_at: String,
    pub db_id: String,
    #[serde(default)]
    pub from: Option<EmailContactInfo>,
    #[serde(default)]
    pub global_id: Option<String>,
    pub has_attachments: bool,
    #[serde(default)]
    pub headers_json: Option<serde_json::Value>,
    #[serde(default)]
    pub internal_date_ts: Option<String>,
    pub is_draft: bool,
    pub is_read: bool,
    pub is_sent: bool,
    pub is_starred: bool,
    #[serde(default)]
    pub labels: Vec<EmailMessageLabel>,
    pub link_id: String,
    #[serde(default)]
    pub provider_history_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_thread_id: Option<String>,
    #[serde(default)]
    pub replying_to_id: Option<String>,
    #[serde(default)]
    pub scheduled_send_time: Option<String>,
    #[serde(default)]
    pub sent_at: Option<String>,
    #[serde(default)]
    pub size_estimate: Option<u64>,
    #[serde(default)]
    pub snippet: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    pub thread_db_id: String,
    #[serde(default)]
    pub to: Vec<EmailContactInfo>,
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// The generated email service representation of one fully assembled thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmailThreadDetail {
    pub access_level: EmailAccessLevel,
    pub created_at: String,
    pub db_id: String,
    pub inbox_visible: bool,
    pub is_read: bool,
    #[serde(default)]
    pub latest_inbound_message_ts: Option<String>,
    #[serde(default)]
    pub latest_non_spam_message_ts: Option<String>,
    #[serde(default)]
    pub latest_outbound_message_ts: Option<String>,
    pub link_id: String,
    #[serde(default)]
    pub messages: Vec<EmailMessageDetail>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub provider_id: Option<String>,
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GetEmailThreadResponse {
    pub thread: EmailThreadDetail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ParsedEmailLabelInfo {
    pub name: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParsedEmailMessage {
    #[serde(default)]
    pub bcc: Vec<EmailContactInfo>,
    #[serde(default)]
    pub body_parsed: Option<String>,
    #[serde(default)]
    pub cc: Vec<EmailContactInfo>,
    pub db_id: String,
    #[serde(default)]
    pub from: Option<EmailContactInfo>,
    #[serde(default)]
    pub internal_date_ts: Option<String>,
    #[serde(default)]
    pub labels: Vec<ParsedEmailLabelInfo>,
    pub link_id: String,
    #[serde(default)]
    pub subject: Option<String>,
    pub thread_db_id: String,
    #[serde(default)]
    pub to: Vec<EmailContactInfo>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmailThreadPreviewAttachment {
    #[serde(default)]
    pub content_id: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub filename: Option<String>,
    pub id: String,
    pub message_id: String,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub provider_attachment_id: Option<String>,
    #[serde(default)]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EmailThreadPreviewContact {
    #[serde(default)]
    pub email_address: Option<String>,
    pub id: String,
    pub link_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub sfs_photo_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EmailThreadPreview {
    pub created_at: String,
    pub id: String,
    pub inbox_visible: bool,
    pub is_draft: bool,
    pub is_important: bool,
    pub is_read: bool,
    pub link_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub owner_id: String,
    #[serde(default)]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub sender_email: Option<String>,
    #[serde(default)]
    pub sender_name: Option<String>,
    #[serde(default)]
    pub sender_photo_url: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
    pub sort_ts: String,
    pub updated_at: String,
    #[serde(default)]
    pub viewed_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<EmailThreadPreviewAttachment>,
    #[serde(default)]
    pub contacts: Vec<EmailThreadPreviewContact>,
    #[serde(default)]
    pub frecency_score: Option<f64>,
    #[serde(default)]
    pub labels: Vec<ApiEmailLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmailThreadPreviewsResponse {
    #[serde(default)]
    pub items: Vec<EmailThreadPreview>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetEmailAttachmentDocumentIdResponse {
    pub attachment_id: String,
    pub document_id: String,
}

impl EmailAttachment {
    /// Decode the service-provided attachment payload without navigating a
    /// browser or webview to an active `data:` URL. Macro currently returns
    /// RFC 2397 base64 payloads, while accepting percent-encoded payloads keeps
    /// the boundary forward-compatible with the same URL format.
    pub fn decoded_data(&self) -> Result<Option<Vec<u8>>> {
        let Some(data_url) = self.data_url.as_deref() else {
            return Ok(None);
        };
        let payload = decode_data_url(data_url)?;
        if let Some(expected) = self.size_bytes {
            anyhow::ensure!(
                payload.len() as u64 == expected,
                "email attachment size mismatch: expected {expected} bytes, decoded {}",
                payload.len()
            );
        }
        Ok(Some(payload))
    }
}

fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let value = data_url
        .strip_prefix("data:")
        .context("email attachment payload is not a data URL")?;
    let (metadata, encoded) = value
        .split_once(',')
        .context("email attachment data URL has no payload separator")?;
    let base64_encoded = metadata
        .split(';')
        .skip(1)
        .any(|parameter| parameter.eq_ignore_ascii_case("base64"));
    if base64_encoded {
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("email attachment data URL contains invalid base64")
    } else {
        Ok(urlencoding::decode_binary(encoded.as_bytes()).into_owned())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetEmailAttachmentResponse {
    pub attachment: EmailAttachment,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct EmailSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature_on_replies_forwards: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmailProvider {
    #[serde(rename = "GMAIL")]
    Gmail,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EmailSyncStatus {
    #[serde(rename = "SYNCING")]
    Syncing,
    #[serde(rename = "UP_TO_DATE")]
    UpToDate,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "NEEDS_REAUTH")]
    NeedsReauth,
    #[serde(rename = "INACTIVE")]
    Inactive,
}

impl EmailSyncStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Syncing => "syncing",
            Self::UpToDate => "up to date",
            Self::Error => "error",
            Self::NeedsReauth => "needs reauth",
            Self::Inactive => "inactive",
        }
    }
}

/// One connected or delegated inbox from Macro's generated email service SDK.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EmailLink {
    pub calendar_disabled: bool,
    pub created_at: String,
    pub email_address: String,
    pub fusionauth_user_id: String,
    pub has_calendar_data: bool,
    pub id: String,
    pub is_primary: bool,
    pub is_sync_active: bool,
    pub macro_id: String,
    pub needs_calendar_permission: bool,
    pub needs_reauth: bool,
    #[serde(default)]
    pub photo_url: Option<String>,
    pub provider: EmailProvider,
    pub settings: EmailSettings,
    pub sync_status: EmailSyncStatus,
    pub updated_at: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListEmailLinksResponse {
    #[serde(default)]
    pub links: Vec<EmailLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchEmailSettingsRequest {
    pub settings: EmailSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchEmailSettingsResponse {
    pub settings: EmailSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitGmailLinkResponse {
    pub authorization_url: String,
    pub link_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitEmailLinkResponse {
    #[serde(default)]
    pub backfill_job_id: Option<String>,
    pub link_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedInboxConflictResponse {
    pub email_address: String,
    pub existing_link_id: String,
    pub existing_owner_email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResyncEmailLinkResponse {
    pub already_in_progress: bool,
    pub backfill_job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiEmailLabel {
    pub created_at: String,
    pub id: String,
    pub label_list_visibility: String,
    pub link_id: String,
    pub message_list_visibility: String,
    pub name: String,
    pub provider_label_id: String,
    #[serde(rename = "type")]
    pub label_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListEmailLabelsResponse {
    #[serde(default)]
    pub labels: Vec<ApiEmailLabel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiEmailFilter {
    pub created_at: String,
    #[serde(default)]
    pub email_address: Option<String>,
    #[serde(default)]
    pub email_domain: Option<String>,
    pub id: String,
    pub is_important: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListEmailFiltersResponse {
    #[serde(default)]
    pub filters: Vec<ApiEmailFilter>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertEmailFilterRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_domain: Option<String>,
    pub is_important: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertEmailFilterResponse {
    pub filter: ApiEmailFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAttendeeInput {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_optional: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAttendee {
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    pub email: String,
    pub is_optional: bool,
    pub is_organizer: bool,
    pub is_self: bool,
    pub response_status: AttendeeResponseStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttendeeResponseStatus {
    NeedsAction,
    Accepted,
    Declined,
    Tentative,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum CalendarEventTime {
    #[serde(rename = "timed", rename_all = "camelCase")]
    Timed {
        starts_at: String,
        ends_at: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time_zone: Option<String>,
    },
    #[serde(rename = "allDay", rename_all = "camelCase")]
    AllDay {
        start_date: String,
        end_date: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CalendarEvent {
    #[serde(default)]
    pub attendees: Vec<CalendarAttendee>,
    #[serde(default)]
    pub calendar_id: Option<String>,
    #[serde(default)]
    pub conference_provider: Option<String>,
    #[serde(default)]
    pub conference_url: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub description: Option<String>,
    pub ical_uid: String,
    pub id: String,
    pub is_read_only: bool,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub organizer_email: Option<String>,
    #[serde(default)]
    pub organizer_name: Option<String>,
    pub owner_id: String,
    #[serde(default)]
    pub recurrence_lines: Vec<String>,
    pub sequence: i64,
    pub status: String,
    pub time: CalendarEventTime,
    pub title: String,
    pub transparency: String,
    pub updated_at: String,
    pub visibility: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VisibleCalendar {
    #[serde(default)]
    pub color: Option<String>,
    pub email_address: String,
    pub email_link_id: String,
    pub id: String,
    pub is_primary: bool,
    pub is_writable: bool,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListCalendarsResponse {
    #[serde(default)]
    pub calendars: Vec<VisibleCalendar>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateCalendarEventRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<CalendarAttendeeInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_link_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recurrence_lines: Vec<String>,
    pub time: CalendarEventTime,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCalendarEventRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attendees: Option<Vec<CalendarAttendeeInput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_lines: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<CalendarEventTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transparency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RsvpCalendarEventRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<String>,
    pub response: AttendeeResponseStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveThreadRequest {
    pub value: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveCallSummary {
    pub call_id: String,
    pub channel_id: String,
    pub created_at: String,
    pub created_by: String,
    pub participant_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveCallsResponse {
    #[serde(default)]
    pub calls: Vec<ActiveCallSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallActiveResponse {
    pub call_id: String,
    pub channel_id: String,
    pub created_at: String,
    pub created_by: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RingStatus {
    Ringing,
    Answered,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RingStatusResponse {
    pub status: RingStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallTokenResponse {
    pub call_id: String,
    pub channel_id: String,
    pub room_name: String,
    pub server_url: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    pub call_id: String,
    pub channel_id: String,
    #[serde(default)]
    pub channel_name: Option<String>,
    pub created_by: String,
    #[serde(default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<f64>,
    #[serde(default)]
    pub egress_id: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    pub is_active: bool,
    #[serde(default)]
    pub participants: Vec<CallRecordParticipant>,
    #[serde(default)]
    pub recording_preview_url: Option<String>,
    #[serde(default)]
    pub recording_started_at: Option<String>,
    #[serde(default)]
    pub recording_url: Option<String>,
    pub room_name: String,
    pub share_with_team: bool,
    pub started_at: String,
    #[serde(default)]
    pub status: Option<CallStatus>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub transcript: Vec<CallRecordTranscriptSegment>,
    #[serde(default)]
    pub user_access_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallRecordParticipant {
    pub joined_at: String,
    #[serde(default)]
    pub left_at: Option<String>,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CallRecordTranscriptSegment {
    pub content: String,
    #[serde(default)]
    pub diarized_speaker_id: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub segment_id: Option<String>,
    pub sequence_num: i64,
    pub speaker_id: String,
    pub started_at: String,
    pub transcript_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CallStatus {
    #[serde(rename = "ATTENDED")]
    Attended,
    #[serde(rename = "MISSED")]
    Missed,
    #[serde(rename = "UNATTENDED")]
    Unattended,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditCallRecordRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_with_team: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CustomSpeakerAssignment {
    pub custom_speaker: Option<String>,
    pub diarized_speaker_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EditCallTranscriptRequest {
    pub assignments: Vec<CustomSpeakerAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LeaveCallResponse {
    pub call_ended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryResponse {
    pub memory: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ImportSource {
    Linear,
    Notion,
    Slack,
}

impl ImportSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Notion => "notion",
            Self::Slack => "slack",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::Notion => "Notion",
            Self::Slack => "Slack",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportEntityStatus {
    Staged,
    Importing,
    Imported,
    Discarded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportInitiator {
    Onboarding,
    Chat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportRunStatus {
    Running,
    Ready,
    Importing,
    Completed,
    Failed,
    Dismissed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImportEntity {
    pub created_at: String,
    #[serde(default)]
    pub entity_id: Option<String>,
    #[serde(default)]
    pub entity_type: Option<String>,
    pub foreign_id: String,
    pub id: String,
    pub initiator: ImportInitiator,
    #[serde(default)]
    pub last_error: Option<String>,
    pub metadata: serde_json::Value,
    pub source: ImportSource,
    pub status: ImportEntityStatus,
    #[serde(default)]
    pub team_id: Option<String>,
    pub updated_at: String,
    pub user_id: String,
}

impl ImportEntity {
    pub fn label(&self) -> &str {
        let field = if self.source == ImportSource::Slack {
            "name"
        } else {
            "title"
        };
        self.metadata
            .get(field)
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("(unnamed)")
    }

    pub fn detail(&self) -> Option<&str> {
        ["description", "summary", "purpose"]
            .into_iter()
            .find_map(|field| self.metadata.get(field)?.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImportRun {
    pub auto_import: bool,
    #[serde(default)]
    pub error: Option<String>,
    pub source: ImportSource,
    pub status: ImportRunStatus,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImportState {
    #[serde(default)]
    pub entities: Vec<ImportEntity>,
    #[serde(default)]
    pub runs: Vec<ImportRun>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OnboardingStatus {
    Active,
    Completed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnboardingRow {
    pub status: OnboardingStatus,
    pub skipped: bool,
    pub started_at: String,
    #[serde(default)]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectedServer {
    pub name: String,
    pub url: String,
    pub authenticated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnboardingState {
    pub row: OnboardingRow,
    #[serde(default)]
    pub connected_servers: Vec<ConnectedServer>,
    #[serde(default)]
    pub suggested_team_domain: Option<String>,
}

/// Deterministic starter documents provisioned by the storage service for the
/// current user. The ids are server-owned: clients must resolve them instead
/// of creating local stand-ins when onboarding is resumed on another device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StarterDocumentsResponse {
    pub how_to_guide_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompleteOnboardingRequest {
    #[serde(default)]
    pub skipped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserTutorialRequest {
    pub tutorial_complete: bool,
}

fn account_deletion_feedback_payload(
    api_key: &str,
    distinct_id: &str,
    reason: &str,
    feedback: Option<&str>,
) -> serde_json::Value {
    let mut properties = serde_json::json!({
        "distinct_id": distinct_id,
        "reason": reason,
        "$lib": "macro-gpui",
    });
    if let Some(feedback) = feedback.filter(|feedback| !feedback.is_empty()) {
        properties["feedback"] = serde_json::Value::String(feedback.into());
    }
    serde_json::json!({
        "api_key": api_key,
        "event": "account_deletion_feedback",
        "properties": properties,
    })
}

impl ImportState {
    pub fn anything_in_flight(&self) -> bool {
        self.runs.iter().any(|run| {
            matches!(
                run.status,
                ImportRunStatus::Running | ImportRunStatus::Importing
            )
        }) || self
            .entities
            .iter()
            .any(|entity| entity.status == ImportEntityStatus::Importing)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunImportRequest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discard_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub import_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunImportOutcome {
    pub discarded: usize,
    pub importing: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddMcpServerRequest {
    pub server_name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateMcpServerRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpServerResponse {
    pub authenticated: bool,
    pub enabled: bool,
    pub server_name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartMcpAuthRequest {
    pub server_name: String,
    pub server_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartMcpAuthResponse {
    pub authorization_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipedreamConnectionResponse {
    pub app_slug: String,
    pub enabled: bool,
    pub server_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipedreamCatalogEntryResponse {
    pub app_slug: String,
    #[serde(default)]
    pub description: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub icon_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipedreamCatalogResponse {
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub servers: Vec<PipedreamCatalogEntryResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PipedreamTokenResponse {
    pub connect_link_url: String,
    pub expires_at: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BotOwner {
    User { user_id: String },
    Team { team_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BotKind {
    Owned,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bot {
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub handle: String,
    pub has_agent: bool,
    pub id: String,
    pub kind: BotKind,
    pub name: String,
    #[serde(default)]
    pub owner: Option<BotOwner>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotChannel {
    pub channel_id: String,
    pub channel_type: String,
    pub joined_at: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BotToken {
    pub bot_id: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    pub id: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    #[serde(default)]
    pub revoked_at: Option<String>,
    pub token_prefix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChannelScopedBotRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_agent: Option<bool>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateChannelScopedBotResponse {
    pub bot: Bot,
    pub bot_token: String,
    pub token: BotToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateBotRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,
    pub name: String,
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_agent: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchBotRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_agent: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateBotTokenRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateBotTokenResponse {
    pub token: BotToken,
    pub bearer_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WebhookFilter {
    pub events: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebhookScope {
    User,
    Team,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebhookStatus {
    Active,
    Paused,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Webhook {
    pub created_at: String,
    pub created_by_user_id: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
    pub endpoint_url: String,
    #[serde(default)]
    pub filters: Vec<WebhookFilter>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub id: String,
    pub is_valid: bool,
    pub name: String,
    pub namespace: String,
    pub status: WebhookStatus,
    pub updated_at: String,
    pub workspace_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListWebhooksResponse {
    #[serde(default)]
    pub webhooks: Vec<Webhook>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateWebhookRequest {
    pub endpoint_url: String,
    pub filters: Vec<WebhookFilter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    pub name: String,
    pub namespace: String,
    pub scope: WebhookScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateWebhookResponse {
    #[serde(flatten)]
    pub webhook: Webhook,
    pub signing_secret: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchWebhookRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<WebhookFilter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<WebhookStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidateWebhookResponse {
    pub is_valid: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub response_status: Option<u16>,
    pub webhook_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Team {
    pub allow_non_admin_invites: bool,
    #[serde(default)]
    pub auto_join_domain: Option<String>,
    pub crm_enabled: bool,
    #[serde(default)]
    pub default_link_share: Option<String>,
    pub enterprise: bool,
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub slug: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    Member,
    Admin,
    Owner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamMember {
    pub role: TeamRole,
    pub team_id: String,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamWithMembers {
    #[serde(default)]
    pub members: Vec<TeamMember>,
    pub team: Team,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamInviteDetails {
    pub created_at: String,
    pub email: String,
    pub id: String,
    pub invited_by: String,
    pub last_sent_at: String,
    pub team_id: String,
    pub team_role: TeamRole,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TeamInvitesResponse {
    #[serde(default)]
    pub invites: Vec<TeamInviteDetails>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateTeamRequest {
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchTeamRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_link_share: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_role_updates: Option<Vec<PatchTeamUserRole>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PatchTeamUserRole {
    pub role: TeamRole,
    pub team_user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InviteToTeamRequest {
    pub invites: Vec<InviteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InviteEntry {
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserName {
    #[serde(default)]
    pub first_name: Option<String>,
    pub id: String,
    #[serde(default)]
    pub last_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserNamesResponse {
    #[serde(default)]
    pub names: Vec<UserName>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserProfilePicture {
    #[serde(default)]
    pub checksum: Option<String>,
    pub id: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfilePictures {
    #[serde(default)]
    pub pictures: Vec<UserProfilePicture>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetProfilePicturesRequestBody {
    pub user_id_list: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserIdsRequest {
    pub user_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GetContactsResponse {
    #[serde(default)]
    pub contacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AddContactRequest {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserQuota {
    pub ai_chat_messages: u64,
    pub documents: u64,
    pub max_ai_chat_messages: u64,
    pub max_documents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PatchUserOnboardingRequest {
    pub first_name: String,
    pub industry: String,
    pub last_name: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateCheckoutSessionRequest {
    pub cancel_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discount: Option<String>,
    pub success_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatePortalSessionRequest {
    pub return_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StripeSessionResponse {
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubConnectionState {
    Disconnected,
    Connected,
    ReauthenticationRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubLinkStatusResponse {
    pub reauthentication_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InitGithubLinkResponse {
    pub authorization_url: String,
    pub link_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BranchNameResponse {
    pub short_id: String,
    pub branch_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestCheckRun {
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub started_at: Option<String>,
    pub status: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestComment {
    #[serde(default)]
    pub author_association: Option<String>,
    #[serde(default)]
    pub author_login: Option<String>,
    pub body: String,
    #[serde(default)]
    pub created_at: Option<String>,
    pub id: i64,
    #[serde(default)]
    pub in_reply_to_id: Option<i64>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub original_line: Option<i64>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub pull_request_review_id: Option<i64>,
    pub source: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

/// Storage's task association plus optional live data returned by auth's
/// `/github_pull_requests/enrich` proxy. Unknown lifecycle values remain
/// strings so a newly introduced GitHub state never prevents task rendering.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequest {
    #[serde(default)]
    pub additions: Option<i64>,
    #[serde(default)]
    pub author_login: Option<String>,
    #[serde(default)]
    pub checks: Option<Vec<GithubPullRequestCheckRun>>,
    #[serde(default)]
    pub comments: Option<Vec<GithubPullRequestComment>>,
    #[serde(default)]
    pub deletions: Option<i64>,
    #[serde(default)]
    pub description: Option<String>,
    pub display_name: String,
    #[serde(default)]
    pub foreign_entity_id: Option<String>,
    pub github_key: String,
    #[serde(default)]
    pub name: Option<String>,
    pub number: i64,
    pub owner: String,
    #[serde(default)]
    pub participant_github_user_ids: Option<Vec<String>>,
    pub repo: String,
    #[serde(default)]
    pub status: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GithubPullRequestsResponse {
    pub pull_requests: Vec<GithubPullRequest>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct GithubPullRequestRef {
    display_name: String,
    github_key: String,
    number: i64,
    owner: String,
    repo: String,
    url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct EnrichGithubPullRequestsRequest {
    pull_requests: Vec<GithubPullRequestRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CursorApiKeyStatus {
    #[serde(default)]
    pub default_model_id: Option<String>,
    pub registered: bool,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CursorModelOption {
    pub display_name: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorModelsResponse {
    #[serde(default)]
    pub models: Vec<CursorModelOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PutCursorApiKeyRequest {
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PutCursorDefaultModelRequest {
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StorageProject {
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub deleted_at: Option<String>,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectsResponse {
    #[serde(default)]
    pub data: Vec<StorageProject>,
    pub error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateProjectRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_parent_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateProjectResponse {
    pub data: StorageProject,
    pub error: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PatchProjectRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_parent_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub share_permission: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectContentResponse {
    #[serde(default)]
    pub data: Vec<serde_json::Value>,
    pub error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentSessionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread: Option<CreateAgentSessionThread>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentSessionThread {
    pub channel_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub message_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentSessionResponse {
    pub session: AgentSessionResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionResponse {
    #[serde(default)]
    pub acp_session_id: Option<String>,
    pub bot_id: String,
    pub created_at: String,
    #[serde(default)]
    pub external: Option<ExternalAgentSession>,
    pub harness: String,
    pub id: String,
    pub model: String,
    pub modified_at: String,
    pub name: String,
    #[serde(default)]
    pub originating_message_id: Option<String>,
    pub owner_id: String,
    #[serde(default)]
    pub repo_url: Option<String>,
    pub sandbox_size: SandboxSize,
    pub status: AgentSessionStatus,
    #[serde(default)]
    pub thread_channel_id: Option<String>,
    #[serde(default)]
    pub thread_id: Option<String>,
    pub workspace: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalAgentSession {
    #[serde(default)]
    pub name: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentSessionStatus {
    NoMessages,
    Event { event: String },
    Disconnected,
}

impl AgentSessionStatus {
    pub fn label(&self) -> String {
        match self {
            Self::NoMessages => "Ready".into(),
            Self::Disconnected => "Disconnected".into(),
            Self::Event { event } => match event.as_str() {
                "acp_ready" => "Ready".into(),
                "acp_starting" => "Starting".into(),
                "acp_stopped" => "Stopped".into(),
                value => value.replace('_', " "),
            },
        }
    }

    pub fn can_resume(&self) -> bool {
        matches!(self, Self::Disconnected)
            || matches!(self, Self::Event { event } if event == "acp_stopped")
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SandboxSize {
    Small,
    Default,
    Large,
}

impl SandboxSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::Small => "Small",
            Self::Default => "Default",
            Self::Large => "Large",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SandboxSizeBody {
    pub size: SandboxSize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum AgentAction {
    #[serde(rename = "prompt")]
    Prompt { prompt: String },
    #[serde(rename = "setModel")]
    SetModel { model: String },
    #[serde(rename = "compact")]
    Compact,
    #[serde(rename = "stop")]
    Stop,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionLogResponse {
    pub bot: AgentSessionBot,
    #[serde(default)]
    pub entries: Vec<AgentSessionLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionBot {
    #[serde(default)]
    pub avatar_url: Option<String>,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSessionLogEntry {
    pub content: serde_json::Value,
    pub direction: AgentLogDirection,
    pub created_at: String,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentLogDirection {
    ToServer,
    ToRuntime,
}

/// The human-readable transcript derived from Macro's persisted Agent Runtime
/// Protocol log. The fold is performed by Macro's authoritative Rust
/// `agent_fold` state machine—the same implementation used by the web client
/// through WASM—rather than by a display-only JSON heuristic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscript {
    pub bot: AgentSessionBot,
    #[serde(default)]
    pub messages: Vec<AgentTranscriptMessage>,
    pub metadata: AgentTranscriptMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscriptMessage {
    pub turn: u32,
    pub author: AgentTranscriptAuthor,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub parts: Vec<AgentTranscriptPart>,
    #[serde(default)]
    pub stop: Option<AgentStopReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentTranscriptAuthor {
    User {
        #[serde(default)]
        user_id: Option<String>,
    },
    Agent,
}

impl AgentTranscriptAuthor {
    pub fn label<'a>(&self, bot: &'a AgentSessionBot) -> &'a str {
        match self {
            Self::User { .. } => "You",
            Self::Agent => &bot.name,
        }
    }

    pub fn is_user(&self) -> bool {
        matches!(self, Self::User { .. })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentTranscriptPart {
    Text {
        text: String,
    },
    Thought {
        text: String,
    },
    Artifact {
        artifact: AgentArtifact,
    },
    ToolUse {
        id: String,
        label: String,
        status: AgentToolStatus,
        detail: AgentToolDetail,
        #[serde(default)]
        artifacts: Vec<AgentArtifact>,
    },
    Permission {
        tool_call: String,
        #[serde(default)]
        options: Vec<AgentPermissionOption>,
        outcome: AgentPermissionOutcome,
    },
    Control {
        control: AgentControl,
        outcome: AgentControlOutcome,
    },
    Plan {
        #[serde(default)]
        entries: Vec<AgentPlanEntry>,
    },
}

/// A non-text ACP content block retained by the native transcript projection.
///
/// Binary payloads remain available for an explicit preview/download action,
/// but renderers should use the metadata and bounded text preview rather than
/// placing `encoded_data` directly in the GPUI element tree.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentArtifact {
    pub kind: AgentArtifactKind,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
    #[serde(default)]
    pub size: Option<i64>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub encoded_data: Option<String>,
    #[serde(default)]
    pub annotations: Option<serde_json::Value>,
    #[serde(default)]
    pub meta: Option<serde_json::Value>,
    /// Exact payload for a future ACP content kind that this client does not
    /// yet understand. Known kinds use typed fields and leave this unset.
    #[serde(default)]
    pub unknown_payload: Option<serde_json::Value>,
}

impl AgentArtifact {
    pub fn label(&self) -> String {
        self.title
            .as_ref()
            .or(self.name.as_ref())
            .or(self.uri.as_ref())
            .cloned()
            .unwrap_or_else(|| self.kind.label().into())
    }

    pub fn encoded_len(&self) -> Option<usize> {
        self.encoded_data.as_ref().map(String::len)
    }

    pub fn decoded_data(&self) -> Result<Option<Vec<u8>>> {
        let Some(encoded) = self.encoded_data.as_deref() else {
            return Ok(None);
        };
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("agent artifact contains invalid base64")
            .map(Some)
    }

    pub fn bounded_image_data_url(&self, max_encoded_bytes: usize) -> Result<Option<String>> {
        if self.kind != AgentArtifactKind::Image {
            return Ok(None);
        }
        let Some(encoded) = self.encoded_data.as_deref() else {
            return Ok(None);
        };
        anyhow::ensure!(
            encoded.len() <= max_encoded_bytes,
            "agent image is too large for an inline preview"
        );
        // Decode before handing the payload to GPUI so malformed data can
        // never become an image source. The original encoded bytes remain in
        // the typed transcript for an explicit retry/download action.
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .context("agent image contains invalid base64")?;
        let mime_type = self
            .mime_type
            .as_deref()
            .filter(|mime| mime.starts_with("image/"))
            .unwrap_or("image/png");
        Ok(Some(format!("data:{mime_type};base64,{encoded}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AgentArtifactKind {
    Image,
    Audio,
    ResourceLink,
    TextResource,
    BlobResource,
    Unknown(String),
}

impl AgentArtifactKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Audio => "Audio",
            Self::ResourceLink => "Resource link",
            Self::TextResource => "Embedded text",
            Self::BlobResource => "Embedded file",
            Self::Unknown(_) => "Artifact",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolStatus {
    Pending,
    Running,
    Completed,
    Failed,
}

impl AgentToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentToolDetail {
    Terminal {
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        exit_code: Option<i32>,
    },
    Edit {
        #[serde(default)]
        diffs: Vec<AgentFileDiff>,
    },
    Read {
        #[serde(default)]
        paths: Vec<String>,
    },
    Delete {
        #[serde(default)]
        paths: Vec<String>,
    },
    Move {
        #[serde(default)]
        paths: Vec<String>,
    },
    Search {
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default)]
        output: Option<String>,
    },
    Fetch {
        #[serde(default)]
        output: Option<String>,
    },
    Think {
        #[serde(default)]
        output: Option<String>,
    },
    Other {
        acp_kind: String,
        #[serde(default)]
        output: Option<String>,
        #[serde(default)]
        input: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentFileDiff {
    pub path: String,
    #[serde(default)]
    pub old_text: Option<String>,
    pub new_text: String,
}

impl AgentFileDiff {
    pub fn additions_and_deletions(&self) -> (usize, usize) {
        let old = self.old_text.as_deref().unwrap_or_default();
        let old_lines = old.lines().collect::<std::collections::BTreeSet<_>>();
        let new_lines = self
            .new_text
            .lines()
            .collect::<std::collections::BTreeSet<_>>();
        (
            new_lines.difference(&old_lines).count(),
            old_lines.difference(&new_lines).count(),
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPermissionOption {
    pub id: String,
    pub name: String,
    pub kind: AgentPermissionOptionKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentPermissionOutcome {
    Pending,
    Selected { option_id: String },
    Cancelled,
    Errored,
    Unrecognized,
}

impl AgentPermissionOutcome {
    pub fn label(&self, options: &[AgentPermissionOption]) -> String {
        match self {
            Self::Pending => "Waiting for permission".into(),
            Self::Selected { option_id } => options
                .iter()
                .find(|option| option.id == *option_id)
                .map(|option| option.name.clone())
                .unwrap_or_else(|| "Permission selected".into()),
            Self::Cancelled => "Permission cancelled".into(),
            Self::Errored => "Permission failed".into(),
            Self::Unrecognized => "Permission resolved".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentControl {
    SetModel { model: String },
    Compact,
    Stop,
}

impl AgentControl {
    pub fn label(&self) -> String {
        match self {
            Self::SetModel { model } => format!("Switch model to {model}"),
            Self::Compact => "Compact context".into(),
            Self::Stop => "Stop agent".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentControlOutcome {
    Pending,
    Accepted,
    Rejected { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentPlanEntry {
    pub content: String,
    pub priority: AgentPlanPriority,
    pub status: AgentPlanStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanPriority {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentPlanStatus {
    Pending,
    InProgress,
    Completed,
}

impl AgentPlanStatus {
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::InProgress => "◐",
            Self::Completed => "●",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
    Other { reason: String },
    Failed { message: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentTranscriptMetadata {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub supported_models: Vec<AgentModelOption>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub available_commands: Vec<AgentAvailableCommand>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentAvailableCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_hint: Option<String>,
}

/// Fold a raw agent session log into Macro's renderable transcript vocabulary.
pub fn fold_agent_session_log(
    _session_id: &str,
    response: &AgentSessionLogResponse,
) -> Result<AgentTranscript> {
    let mut machine = NativeAgentFold::default();
    for entry in &response.entries {
        machine.push(entry);
    }
    Ok(AgentTranscript {
        bot: response.bot.clone(),
        messages: machine.messages,
        metadata: machine.metadata,
    })
}

/// A native, total fold over Macro's Agent Runtime Protocol frames.
///
/// This mirrors `macro/crates/agent_fold`: understood frames update typed
/// transcript state; unknown future frames remain available in the raw log
/// and are ignored instead of preventing the rest of a session from loading.
/// Keeping the projection independent of a particular ACP crate release lets
/// Zed and Macro update their ACP SDKs on different schedules.
#[derive(Default)]
struct NativeAgentFold {
    messages: Vec<AgentTranscriptMessage>,
    metadata: AgentTranscriptMetadata,
    turn: Option<NativeAgentTurn>,
    next_turn: u32,
    pending_permissions: HashMap<String, String>,
    pending_controls: HashMap<String, (usize, usize)>,
    pending_config_requests: BTreeSet<String>,
}

struct NativeAgentTurn {
    id: u32,
    prompt_id: Option<String>,
    agent_message: Option<usize>,
    tool_positions: HashMap<String, usize>,
    permission_positions: HashMap<String, usize>,
    plan_position: Option<usize>,
}

impl NativeAgentFold {
    fn push(&mut self, entry: &AgentSessionLogEntry) {
        match entry.direction {
            AgentLogDirection::ToRuntime => {
                self.to_runtime(&entry.content, entry.user_id.clone());
            }
            AgentLogDirection::ToServer => self.to_server(&entry.content),
        }
    }

    fn to_runtime(&mut self, frame: &serde_json::Value, user_id: Option<String>) {
        if frame.get("type").and_then(serde_json::Value::as_str) != Some("acp") {
            return;
        }
        let method = frame
            .get("method")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let id = frame.get("id").and_then(json_rpc_id);
        let params = frame.get("params");

        match method {
            "session/prompt" => {
                let parts = params
                    .and_then(|params| params.get("prompt"))
                    .and_then(serde_json::Value::as_array)
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(content_block_part)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let is_compact = matches!(parts.as_slice(), [AgentTranscriptPart::Text { text }] if text.trim() == "/compact");
                if is_compact {
                    self.begin_compact(id, user_id);
                } else {
                    self.begin_turn(id, parts, user_id);
                }
            }
            "session/set_config_option" => {
                if let Some(id) = id.clone() {
                    self.pending_config_requests.insert(id);
                }
                let is_model = params
                    .and_then(|params| params.get("configId"))
                    .and_then(serde_json::Value::as_str)
                    == Some("model");
                if is_model {
                    let model = params
                        .and_then(|params| params.get("value"))
                        .and_then(value_id)
                        .unwrap_or_else(|| "default".into());
                    self.record_control(AgentControl::SetModel { model }, id, user_id);
                }
            }
            "session/new" | "session/load" | "session/resume" => {
                if let Some(id) = id {
                    self.pending_config_requests.insert(id);
                }
            }
            "session/cancel" => self.record_control(AgentControl::Stop, None, user_id),
            _ => {
                if let Some(id) = frame.get("id").and_then(json_rpc_id) {
                    if let Some(result) = frame.get("result") {
                        self.resolve_permission(&id, Some(result));
                    } else if frame.get("error").is_some() {
                        self.resolve_permission(&id, None);
                    }
                }
            }
        }
    }

    fn to_server(&mut self, frame: &serde_json::Value) {
        match frame.get("type").and_then(serde_json::Value::as_str) {
            Some("event") => {
                let event = frame
                    .get("event")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned);
                if event.as_deref() == Some("acp_ready") {
                    self.pending_permissions.clear();
                    self.pending_controls.clear();
                    self.pending_config_requests.clear();
                }
                self.metadata.status = event;
            }
            Some("acp") => {
                let method = frame
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                match method {
                    "session/update" => {
                        if let Some(update) = frame.pointer("/params/update") {
                            self.session_update(update);
                        }
                    }
                    "session/request_permission" => {
                        if let Some(id) = frame.get("id").and_then(json_rpc_id) {
                            self.request_permission(&id, frame.get("params"));
                        }
                    }
                    _ => {
                        let Some(id) = frame.get("id").and_then(json_rpc_id) else {
                            return;
                        };
                        let error = frame
                            .pointer("/error/message")
                            .and_then(serde_json::Value::as_str);
                        if self.resolve_control(&id, error) {
                            self.pending_config_requests.remove(&id);
                            return;
                        }
                        if self.pending_config_requests.remove(&id) {
                            if error.is_none() {
                                self.apply_config_options(
                                    frame
                                        .pointer("/result/configOptions")
                                        .and_then(serde_json::Value::as_array),
                                );
                            }
                            return;
                        }
                        match error {
                            Some(message) => self.fail_turn(&id, message),
                            None => self.end_turn(&id, frame.get("result")),
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn begin_turn(
        &mut self,
        prompt_id: Option<String>,
        parts: Vec<AgentTranscriptPart>,
        user_id: Option<String>,
    ) {
        self.close_turn(None);
        let turn = self.next_turn;
        self.next_turn += 1;
        if !parts.is_empty() {
            self.messages.push(AgentTranscriptMessage {
                turn,
                author: AgentTranscriptAuthor::User { user_id },
                request_id: prompt_id
                    .as_ref()
                    .filter(|id| id.starts_with("agent_session:"))
                    .cloned(),
                parts,
                stop: None,
            });
        }
        self.turn = Some(NativeAgentTurn {
            id: turn,
            prompt_id,
            agent_message: None,
            tool_positions: HashMap::new(),
            permission_positions: HashMap::new(),
            plan_position: None,
        });
    }

    fn begin_compact(&mut self, prompt_id: Option<String>, user_id: Option<String>) {
        self.close_turn(None);
        let turn = self.next_turn;
        self.next_turn += 1;
        self.messages.push(AgentTranscriptMessage {
            turn,
            author: AgentTranscriptAuthor::User { user_id },
            request_id: prompt_id
                .as_ref()
                .filter(|id| id.starts_with("agent_session:"))
                .cloned(),
            parts: vec![AgentTranscriptPart::Control {
                control: AgentControl::Compact,
                outcome: AgentControlOutcome::Accepted,
            }],
            stop: None,
        });
        self.turn = Some(NativeAgentTurn {
            id: turn,
            prompt_id,
            agent_message: None,
            tool_positions: HashMap::new(),
            permission_positions: HashMap::new(),
            plan_position: None,
        });
    }

    fn record_control(
        &mut self,
        control: AgentControl,
        request_id: Option<String>,
        user_id: Option<String>,
    ) {
        let turn = self.next_turn;
        self.next_turn += 1;
        let outcome = if request_id.is_some() {
            AgentControlOutcome::Pending
        } else {
            AgentControlOutcome::Accepted
        };
        let message = self.messages.len();
        self.messages.push(AgentTranscriptMessage {
            turn,
            author: AgentTranscriptAuthor::User { user_id },
            request_id: request_id
                .as_ref()
                .filter(|id| id.starts_with("agent_session:"))
                .cloned(),
            parts: vec![AgentTranscriptPart::Control { control, outcome }],
            stop: None,
        });
        if let Some(request_id) = request_id {
            self.pending_controls.insert(request_id, (message, 0));
        }
    }

    fn resolve_control(&mut self, id: &str, error: Option<&str>) -> bool {
        let Some((message, part)) = self.pending_controls.remove(id) else {
            return false;
        };
        if let Some(AgentTranscriptPart::Control { outcome, .. }) = self
            .messages
            .get_mut(message)
            .and_then(|message| message.parts.get_mut(part))
        {
            *outcome = match error {
                Some(message) => AgentControlOutcome::Rejected {
                    message: message.into(),
                },
                None => AgentControlOutcome::Accepted,
            };
        }
        true
    }

    fn end_turn(&mut self, response_id: &str, result: Option<&serde_json::Value>) {
        let stop = result
            .and_then(|result| result.get("stopReason"))
            .and_then(serde_json::Value::as_str)
            .map(agent_stop_reason);
        let matches = self.turn.as_ref().is_some_and(|turn| {
            turn.prompt_id
                .as_deref()
                .map(|prompt| prompt == response_id)
                .unwrap_or(stop.is_some())
        });
        if matches {
            self.close_turn(stop);
        }
    }

    fn fail_turn(&mut self, response_id: &str, error: &str) {
        let matches = self
            .turn
            .as_ref()
            .and_then(|turn| turn.prompt_id.as_deref())
            == Some(response_id);
        if !matches {
            return;
        }
        let message = self.ensure_agent_message(AgentTranscriptPart::Text {
            text: String::new(),
        });
        self.turn = None;
        if let Some(message) = message.and_then(|index| self.messages.get_mut(index)) {
            message.stop = Some(AgentStopReason::Failed {
                message: error.into(),
            });
        }
    }

    fn close_turn(&mut self, stop: Option<AgentStopReason>) {
        if let Some(turn) = self.turn.take()
            && let (Some(message), Some(stop)) = (turn.agent_message, stop)
            && let Some(message) = self.messages.get_mut(message)
        {
            message.stop = Some(stop);
        }
    }

    fn session_update(&mut self, update: &serde_json::Value) {
        match update
            .get("sessionUpdate")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
        {
            "agent_message_chunk" => {
                if let Some(content) = update.get("content") {
                    if let Some(text) = content_block_text(content) {
                        self.append_text(text, false);
                    } else if let Some(artifact) = content_block_artifact(content) {
                        self.push_agent_part(AgentTranscriptPart::Artifact { artifact });
                    }
                }
            }
            "agent_thought_chunk" => {
                if let Some(content) = update.get("content") {
                    if let Some(text) = content_block_text(content) {
                        self.append_text(text, true);
                    } else if let Some(artifact) = content_block_artifact(content) {
                        self.push_agent_part(AgentTranscriptPart::Artifact { artifact });
                    }
                }
            }
            // The prompt request is authoritative; runtimes may echo it.
            "user_message_chunk" => {}
            "tool_call" => self.open_tool_call(update),
            "tool_call_update" => self.patch_tool_call(update),
            "plan" => self.apply_plan(update),
            "session_info_update" => {
                if update.get("title").is_some() {
                    self.metadata.title = update
                        .get("title")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned);
                }
            }
            "config_option_update" => {
                self.apply_config_options(
                    update
                        .get("configOptions")
                        .and_then(serde_json::Value::as_array),
                );
            }
            "available_commands_update" => {
                self.metadata.available_commands = update
                    .get("availableCommands")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|command| {
                        Some(AgentAvailableCommand {
                            name: command.get("name")?.as_str()?.into(),
                            description: command
                                .get("description")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .into(),
                            input_hint: command
                                .pointer("/input/hint")
                                .and_then(serde_json::Value::as_str)
                                .map(ToOwned::to_owned),
                        })
                    })
                    .collect();
            }
            // Usage and mode are deliberately not transcript content.
            _ => {}
        }
    }

    fn append_text(&mut self, text: String, thought: bool) {
        if let Some(message) = self.agent_message_mut()
            && let Some(part) = message.parts.last_mut()
        {
            match (thought, part) {
                (false, AgentTranscriptPart::Text { text: existing })
                | (true, AgentTranscriptPart::Thought { text: existing }) => {
                    existing.push_str(&text);
                    return;
                }
                _ => {}
            }
        }
        self.push_agent_part(if thought {
            AgentTranscriptPart::Thought { text }
        } else {
            AgentTranscriptPart::Text { text }
        });
    }

    fn open_tool_call(&mut self, update: &serde_json::Value) {
        let Some(id) = update
            .get("toolCallId")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let label = tool_name(update).unwrap_or_else(|| {
            update
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Tool")
                .into()
        });
        let part = AgentTranscriptPart::ToolUse {
            id: id.clone(),
            label,
            status: tool_status(update.get("status")),
            detail: tool_detail(update),
            artifacts: tool_artifacts(update),
        };
        let existing = self
            .turn
            .as_ref()
            .and_then(|turn| turn.tool_positions.get(&id).copied());
        if let Some(position) = existing {
            if let Some(part_slot) = self
                .agent_message_mut()
                .and_then(|message| message.parts.get_mut(position))
            {
                *part_slot = part;
            }
            return;
        }
        if let Some(position) = self.push_agent_part(part)
            && let Some(turn) = self.turn.as_mut()
        {
            turn.tool_positions.insert(id, position);
        }
    }

    fn patch_tool_call(&mut self, update: &serde_json::Value) {
        let Some(id) = update.get("toolCallId").and_then(serde_json::Value::as_str) else {
            return;
        };
        let Some(position) = self
            .turn
            .as_ref()
            .and_then(|turn| turn.tool_positions.get(id).copied())
        else {
            return;
        };
        let Some(AgentTranscriptPart::ToolUse {
            label,
            status,
            detail,
            artifacts,
            ..
        }) = self
            .agent_message_mut()
            .and_then(|message| message.parts.get_mut(position))
        else {
            return;
        };
        if update.get("status").is_some() {
            *status = tool_status(update.get("status"));
        }
        if let Some(name) = tool_name(update) {
            *label = name;
        } else if label.is_empty()
            && let Some(title) = update.get("title").and_then(serde_json::Value::as_str)
        {
            *label = title.into();
        }
        patch_tool_detail(detail, update);
        if update.get("content").is_some() {
            *artifacts = tool_artifacts(update);
        }
    }

    fn apply_plan(&mut self, update: &serde_json::Value) {
        let entries = update
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                Some(AgentPlanEntry {
                    content: entry.get("content")?.as_str()?.into(),
                    priority: match entry.get("priority").and_then(serde_json::Value::as_str) {
                        Some("high") => AgentPlanPriority::High,
                        Some("low") => AgentPlanPriority::Low,
                        _ => AgentPlanPriority::Medium,
                    },
                    status: match entry.get("status").and_then(serde_json::Value::as_str) {
                        Some("in_progress") => AgentPlanStatus::InProgress,
                        Some("completed") => AgentPlanStatus::Completed,
                        _ => AgentPlanStatus::Pending,
                    },
                })
            })
            .collect::<Vec<_>>();
        if let Some(position) = self.turn.as_ref().and_then(|turn| turn.plan_position) {
            if let Some(AgentTranscriptPart::Plan { entries: existing }) = self
                .agent_message_mut()
                .and_then(|message| message.parts.get_mut(position))
            {
                *existing = entries;
            }
        } else if !entries.is_empty()
            && let Some(position) = self.push_agent_part(AgentTranscriptPart::Plan { entries })
            && let Some(turn) = self.turn.as_mut()
        {
            turn.plan_position = Some(position);
        }
    }

    fn request_permission(&mut self, request_id: &str, params: Option<&serde_json::Value>) {
        let Some(params) = params else { return };
        let Some(tool_call) = params
            .pointer("/toolCall/toolCallId")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
        else {
            return;
        };
        let options = params
            .get("options")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                Some(AgentPermissionOption {
                    id: option.get("optionId")?.as_str()?.into(),
                    name: option
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("Choose")
                        .into(),
                    kind: match option.get("kind").and_then(serde_json::Value::as_str) {
                        Some("allow_once") => AgentPermissionOptionKind::AllowOnce,
                        Some("allow_always") => AgentPermissionOptionKind::AllowAlways,
                        Some("reject_always") => AgentPermissionOptionKind::RejectAlways,
                        _ => AgentPermissionOptionKind::RejectOnce,
                    },
                })
            })
            .collect();
        self.pending_permissions
            .insert(request_id.into(), tool_call.clone());
        if let Some(position) = self.push_agent_part(AgentTranscriptPart::Permission {
            tool_call: tool_call.clone(),
            options,
            outcome: AgentPermissionOutcome::Pending,
        }) && let Some(turn) = self.turn.as_mut()
        {
            turn.permission_positions.insert(tool_call, position);
        }
    }

    fn resolve_permission(&mut self, response_id: &str, result: Option<&serde_json::Value>) {
        let Some(tool_call) = self.pending_permissions.remove(response_id) else {
            return;
        };
        let outcome = match result {
            None => AgentPermissionOutcome::Errored,
            Some(result) => match result
                .pointer("/outcome/outcome")
                .and_then(serde_json::Value::as_str)
            {
                Some("selected") => result
                    .pointer("/outcome/optionId")
                    .and_then(serde_json::Value::as_str)
                    .map(|option_id| AgentPermissionOutcome::Selected {
                        option_id: option_id.into(),
                    })
                    .unwrap_or(AgentPermissionOutcome::Unrecognized),
                Some("cancelled") => AgentPermissionOutcome::Cancelled,
                _ => AgentPermissionOutcome::Unrecognized,
            },
        };
        let Some(position) = self
            .turn
            .as_ref()
            .and_then(|turn| turn.permission_positions.get(&tool_call).copied())
        else {
            return;
        };
        if let Some(AgentTranscriptPart::Permission {
            outcome: existing, ..
        }) = self
            .agent_message_mut()
            .and_then(|message| message.parts.get_mut(position))
        {
            *existing = outcome;
        }
    }

    fn apply_config_options(&mut self, options: Option<&Vec<serde_json::Value>>) {
        let Some(model) = options
            .into_iter()
            .flatten()
            .find(|option| option.get("id").and_then(serde_json::Value::as_str) == Some("model"))
        else {
            return;
        };
        self.metadata.model = model.get("currentValue").and_then(value_id);
        self.metadata.supported_models = model
            .get("options")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                Some(AgentModelOption {
                    id: option.get("value").and_then(value_id)?,
                    name: option
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    description: option
                        .get("description")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                })
            })
            .collect();
    }

    fn ensure_agent_message(&mut self, first: AgentTranscriptPart) -> Option<usize> {
        if self.turn.is_none() {
            let id = self.next_turn;
            self.next_turn += 1;
            self.turn = Some(NativeAgentTurn {
                id,
                prompt_id: None,
                agent_message: None,
                tool_positions: HashMap::new(),
                permission_positions: HashMap::new(),
                plan_position: None,
            });
        }
        if let Some(index) = self.turn.as_ref()?.agent_message {
            return Some(index);
        }
        let turn = self.turn.as_ref()?.id;
        let index = self.messages.len();
        self.messages.push(AgentTranscriptMessage {
            turn,
            author: AgentTranscriptAuthor::Agent,
            request_id: None,
            parts: vec![first],
            stop: None,
        });
        self.turn.as_mut()?.agent_message = Some(index);
        Some(index)
    }

    fn push_agent_part(&mut self, part: AgentTranscriptPart) -> Option<usize> {
        if self
            .turn
            .as_ref()
            .and_then(|turn| turn.agent_message)
            .is_none()
        {
            self.ensure_agent_message(part)?;
            return Some(0);
        }
        let message = self.agent_message_mut()?;
        let position = message.parts.len();
        message.parts.push(part);
        Some(position)
    }

    fn agent_message_mut(&mut self) -> Option<&mut AgentTranscriptMessage> {
        let index = self.turn.as_ref()?.agent_message?;
        self.messages.get_mut(index)
    }
}

fn json_rpc_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn value_id(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        object @ serde_json::Value::Object(_) => object
            .get("value")
            .or_else(|| object.get("id"))
            .and_then(value_id),
        _ => None,
    }
}

fn content_block_text(value: &serde_json::Value) -> Option<String> {
    (value.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .then(|| value.get("text").and_then(serde_json::Value::as_str))
        .flatten()
        .map(ToOwned::to_owned)
}

fn content_block_part(value: &serde_json::Value) -> Option<AgentTranscriptPart> {
    content_block_text(value)
        .map(|text| AgentTranscriptPart::Text { text })
        .or_else(|| {
            content_block_artifact(value).map(|artifact| AgentTranscriptPart::Artifact { artifact })
        })
}

fn content_block_artifact(value: &serde_json::Value) -> Option<AgentArtifact> {
    let content_type = value.get("type")?.as_str()?;
    if content_type == "text" {
        return None;
    }
    let annotations = value.get("annotations").cloned();
    let meta = value.get("_meta").cloned();
    let empty = || AgentArtifact {
        kind: AgentArtifactKind::Unknown(content_type.into()),
        name: None,
        title: None,
        description: None,
        uri: None,
        mime_type: None,
        size: None,
        text: None,
        encoded_data: None,
        annotations: annotations.clone(),
        meta: meta.clone(),
        unknown_payload: None,
    };
    Some(match content_type {
        "image" => AgentArtifact {
            kind: AgentArtifactKind::Image,
            uri: value
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            mime_type: value
                .get("mimeType")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            encoded_data: value
                .get("data")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            ..empty()
        },
        "audio" => AgentArtifact {
            kind: AgentArtifactKind::Audio,
            mime_type: value
                .get("mimeType")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            encoded_data: value
                .get("data")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            ..empty()
        },
        "resource_link" => AgentArtifact {
            kind: AgentArtifactKind::ResourceLink,
            name: value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            title: value
                .get("title")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            description: value
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            uri: value
                .get("uri")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            mime_type: value
                .get("mimeType")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            size: value.get("size").and_then(serde_json::Value::as_i64),
            ..empty()
        },
        "resource" => {
            let resource = value.get("resource")?;
            let resource_meta = resource.get("_meta").cloned().or_else(|| meta.clone());
            if let Some(text) = resource.get("text").and_then(serde_json::Value::as_str) {
                AgentArtifact {
                    kind: AgentArtifactKind::TextResource,
                    uri: resource
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    mime_type: resource
                        .get("mimeType")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    text: Some(text.into()),
                    meta: resource_meta,
                    ..empty()
                }
            } else if let Some(blob) = resource.get("blob").and_then(serde_json::Value::as_str) {
                AgentArtifact {
                    kind: AgentArtifactKind::BlobResource,
                    uri: resource
                        .get("uri")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    mime_type: resource
                        .get("mimeType")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    encoded_data: Some(blob.into()),
                    meta: resource_meta,
                    ..empty()
                }
            } else {
                AgentArtifact {
                    unknown_payload: Some(value.clone()),
                    ..empty()
                }
            }
        }
        _ => AgentArtifact {
            unknown_payload: Some(value.clone()),
            ..empty()
        },
    })
}

fn tool_artifacts(value: &serde_json::Value) -> Vec<AgentArtifact> {
    value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|content| {
            let block =
                if content.get("type").and_then(serde_json::Value::as_str) == Some("content") {
                    content.get("content")?
                } else {
                    content
                };
            content_block_artifact(block)
        })
        .collect()
}

fn agent_stop_reason(reason: &str) -> AgentStopReason {
    match reason {
        "end_turn" => AgentStopReason::EndTurn,
        "max_tokens" => AgentStopReason::MaxTokens,
        "max_turn_requests" => AgentStopReason::MaxTurnRequests,
        "refusal" => AgentStopReason::Refusal,
        "cancelled" => AgentStopReason::Cancelled,
        reason => AgentStopReason::Other {
            reason: reason.into(),
        },
    }
}

fn tool_name(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/_meta/claudeCode/toolName")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn tool_status(value: Option<&serde_json::Value>) -> AgentToolStatus {
    match value.and_then(serde_json::Value::as_str) {
        Some("in_progress" | "running") => AgentToolStatus::Running,
        Some("completed") => AgentToolStatus::Completed,
        Some("failed") => AgentToolStatus::Failed,
        _ => AgentToolStatus::Pending,
    }
}

fn tool_detail(value: &serde_json::Value) -> AgentToolDetail {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("other");
    match kind {
        "execute" => AgentToolDetail::Terminal {
            command: value
                .pointer("/rawInput/command")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            output: terminal_output(value),
            exit_code: terminal_exit_code(value),
        },
        "edit" => AgentToolDetail::Edit {
            diffs: tool_diffs(value),
        },
        "read" => AgentToolDetail::Read {
            paths: tool_paths(value),
        },
        "delete" => AgentToolDetail::Delete {
            paths: tool_paths(value),
        },
        "move" => AgentToolDetail::Move {
            paths: tool_paths(value),
        },
        "search" => AgentToolDetail::Search {
            paths: tool_paths(value),
            output: tool_output(value),
        },
        "fetch" => AgentToolDetail::Fetch {
            output: tool_output(value),
        },
        "think" => AgentToolDetail::Think {
            output: tool_output(value),
        },
        kind => AgentToolDetail::Other {
            acp_kind: kind.into(),
            output: tool_output(value),
            input: value.get("rawInput").cloned(),
        },
    }
}

fn patch_tool_detail(detail: &mut AgentToolDetail, value: &serde_json::Value) {
    match detail {
        AgentToolDetail::Terminal {
            command,
            output,
            exit_code,
        } => {
            if let Some(found) = value
                .pointer("/rawInput/command")
                .and_then(serde_json::Value::as_str)
            {
                *command = Some(found.into());
            }
            if let Some(found) = terminal_output(value) {
                *output = Some(found);
            }
            if let Some(found) = terminal_exit_code(value) {
                *exit_code = Some(found);
            }
        }
        AgentToolDetail::Edit { diffs } => {
            let found = tool_diffs(value);
            if !found.is_empty() {
                *diffs = found;
            }
        }
        AgentToolDetail::Read { paths }
        | AgentToolDetail::Delete { paths }
        | AgentToolDetail::Move { paths } => {
            let found = tool_paths(value);
            if !found.is_empty() {
                *paths = found;
            }
        }
        AgentToolDetail::Search { paths, output } => {
            let found = tool_paths(value);
            if !found.is_empty() {
                *paths = found;
            }
            if let Some(found) = tool_output(value) {
                *output = Some(found);
            }
        }
        AgentToolDetail::Fetch { output } | AgentToolDetail::Think { output } => {
            if let Some(found) = tool_output(value) {
                *output = Some(found);
            }
        }
        AgentToolDetail::Other { input, output, .. } => {
            if let Some(found) = value.get("rawInput") {
                *input = Some(found.clone());
            }
            if let Some(found) = tool_output(value) {
                *output = Some(found);
            }
        }
    }
}

fn tool_paths(value: &serde_json::Value) -> Vec<String> {
    value
        .get("locations")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|location| {
            location
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn tool_diffs(value: &serde_json::Value) -> Vec<AgentFileDiff> {
    let mut diffs = value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|content| content.get("type").and_then(serde_json::Value::as_str) == Some("diff"))
        .filter_map(|diff| {
            Some(AgentFileDiff {
                path: diff.get("path")?.as_str()?.into(),
                old_text: diff
                    .get("oldText")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned),
                new_text: diff
                    .get("newText")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .into(),
            })
        })
        .collect::<Vec<_>>();
    if diffs.is_empty()
        && let (Some(path), Some(new_text)) = (
            value
                .pointer("/rawInput/filePath")
                .and_then(serde_json::Value::as_str),
            value
                .pointer("/rawInput/content")
                .and_then(serde_json::Value::as_str),
        )
    {
        diffs.push(AgentFileDiff {
            path: path.into(),
            old_text: None,
            new_text: new_text.into(),
        });
    }
    diffs
}

fn tool_output(value: &serde_json::Value) -> Option<String> {
    let output = value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|content| {
            let block =
                if content.get("type").and_then(serde_json::Value::as_str) == Some("content") {
                    content.get("content")?
                } else {
                    content
                };
            content_block_text(block)
        })
        .collect::<String>();
    (!output.is_empty()).then_some(output)
}

fn terminal_output(value: &serde_json::Value) -> Option<String> {
    value
        .pointer("/_meta/terminal_output/data")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

fn terminal_exit_code(value: &serde_json::Value) -> Option<i32> {
    value
        .pointer("/_meta/terminal_exit/exit_code")
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScheduledAction {
    #[serde(default)]
    pub claimed: Option<String>,
    pub created_at: String,
    pub enabled: bool,
    #[serde(default)]
    pub id: Option<String>,
    pub kind: ScheduledActionKind,
    pub name: String,
    pub next_run_at: String,
    pub owner: String,
    pub schedule: String,
    pub task: serde_json::Value,
    pub timezone: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ScheduledActionKind {
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateScheduledAction {
    pub enabled: bool,
    pub kind: ScheduledActionKind,
    pub name: String,
    pub schedule: String,
    pub task: serde_json::Value,
    pub timezone: String,
}

pub type UpdateScheduledAction = CreateScheduledAction;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentScheduledTask {
    pub model: String,
    pub prompt: String,
    pub user_prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InProgressExecution {
    pub action_id: String,
    #[serde(default)]
    pub chat_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionExecutionRecord {
    pub action_id: String,
    pub created_at: String,
    pub end_time: String,
    #[serde(default)]
    pub id: Option<String>,
    pub is_success: bool,
    #[serde(default)]
    pub resource_id: Option<String>,
    pub result: serde_json::Value,
    pub start_time: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateCrmCompanyRequest {
    pub domain: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateCrmContactRequest {
    pub email: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CrmCompanyResponse {
    #[serde(default)]
    pub contacts: Vec<CrmContactResponse>,
    pub created_at: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub domains: Vec<CrmDomainResponse>,
    pub email_sync: bool,
    pub hidden: bool,
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub team_id: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CrmContactResponse {
    pub company_id: String,
    pub created_at: String,
    pub email: String,
    pub first_interaction: String,
    pub hidden: bool,
    pub id: String,
    pub last_interaction: String,
    #[serde(default)]
    pub name: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CrmDomainResponse {
    pub company_id: String,
    pub created_at: String,
    pub domain: String,
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetCrmNameRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetCrmHiddenRequest {
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetCrmEmailSyncRequest {
    pub email_sync: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CrmCommentEntityType {
    CrmCompany,
    CrmContact,
}

impl CrmCommentEntityType {
    pub fn as_path_segment(self) -> &'static str {
        match self {
            Self::CrmCompany => "crm_company",
            Self::CrmContact => "crm_contact",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CrmCommentThreadInfo {
    pub thread_id: String,
    pub entity_type: CrmCommentEntityType,
    pub entity_id: String,
    pub owner: String,
    pub resolved: bool,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CrmComment {
    pub comment_id: String,
    pub thread_id: String,
    #[serde(default)]
    pub order: Option<i32>,
    pub owner: String,
    #[serde(default)]
    pub sender: Option<String>,
    pub text: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrmCommentThread {
    pub thread: CrmCommentThreadInfo,
    #[serde(default)]
    pub comments: Vec<CrmComment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CreateCrmCommentRequest {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_metadata: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditCrmCommentRequest {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeleteCrmCommentResult {
    pub comment_id: String,
    pub thread_id: String,
    pub thread_deleted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CrmPermissionRole {
    Admin,
    Owner,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CrmTeamSettingsResponse {
    #[serde(default)]
    pub closed_stage_ids: Option<Vec<String>>,
    #[serde(default)]
    pub default_team_view_id: Option<String>,
    pub delete_records_role: CrmPermissionRole,
    pub edit_stages_role: CrmPermissionRole,
    pub move_closed_deals_role: CrmPermissionRole,
    pub team_views: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UpdateCrmTeamSettingsRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_stage_ids: Option<Option<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_team_view_id: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_records_role: Option<CrmPermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_stages_role: Option<CrmPermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub move_closed_deals_role: Option<CrmPermissionRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_views: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub config: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedDefaultView {
    pub id: String,
    pub user_id: String,
    pub default_view_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SavedViewsResponse {
    #[serde(default)]
    pub views: Vec<SavedView>,
    #[serde(default)]
    pub excluded_default_views: Vec<ExcludedDefaultView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreateSavedViewRequest {
    pub name: String,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PatchSavedViewRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExcludeDefaultViewRequest {
    pub default_view_id: String,
}

/// Entity scope used when grouping Soup rows by a property definition.
///
/// This mirrors `ApiPropertyEntityType` in Macro's open-source storage SDK.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SoupPropertyEntityType {
    Channel,
    Chat,
    Company,
    Document,
    Project,
    Task,
    Thread,
    User,
}

/// Server-owned grouping field for `/items/soup/ast/grouped`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SoupGroupByField {
    Date,
    EntityType,
    Project,
    #[serde(rename = "property")]
    Property {
        property_definition_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        entity_type: Option<SoupPropertyEntityType>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupedSoupSort {
    ViewedAt,
    CreatedAt,
    UpdatedAt,
    ViewedUpdated,
}

/// Ordered membership and pagination metadata for one server-owned Soup group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoupGroupMeta {
    #[serde(default)]
    pub display_order: Option<i32>,
    #[serde(default)]
    pub item_ids: Vec<String>,
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub next_cursor: Option<String>,
    pub total_count: u32,
}

/// Typed response from Macro's grouped Soup endpoint. Items remain raw Soup
/// values because the backend's item union is intentionally open-ended.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum GroupedSoupPage {
    Initial {
        #[serde(default)]
        items: HashMap<String, serde_json::Value>,
        #[serde(default)]
        groups: Vec<SoupGroupMeta>,
    },
    GroupPage {
        #[serde(default)]
        items: HashMap<String, serde_json::Value>,
        group: SoupGroupMeta,
    },
}

#[derive(Clone)]
pub struct MacroApi {
    http: Client,
    config: MacroApiConfig,
    token: Arc<parking_lot::RwLock<Option<String>>>,
}

fn channel_messages_path(
    channel_id: &str,
    limit: u32,
    cursor: Option<&str>,
    previous_cursor: Option<&str>,
    load_around_message_id: Option<&str>,
) -> String {
    let mut path = format!(
        "/channels/{}/messages?limit={limit}",
        urlencoding::encode(channel_id)
    );
    for (key, value) in [
        ("cursor", cursor),
        ("previous_cursor", previous_cursor),
        ("load_around_message_id", load_around_message_id),
    ] {
        if let Some(value) = value {
            path.push('&');
            path.push_str(key);
            path.push('=');
            path.push_str(&urlencoding::encode(value));
        }
    }
    path
}

fn grouped_soup_filter_object(
    filters: &serde_json::Value,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let mut filters = filters
        .as_object()
        .context("grouped Soup filters must be a JSON object")?
        .clone();
    // These belong to the flat Soup request. Grouped Soup owns its mode,
    // grouping, sort, limit, and opaque cursor fields independently.
    for key in [
        "cursor",
        "expand",
        "group_by",
        "group_key",
        "limit",
        "mode",
        "per_group_limit",
        "sort_direction",
        "sort_method",
    ] {
        filters.remove(key);
    }
    Ok(filters)
}

impl MacroApi {
    pub fn new(config: MacroApiConfig) -> Self {
        Self {
            http: Client::builder()
                .user_agent("macro-gpui/0.1")
                .build()
                .expect("failed to build Macro HTTP client"),
            config,
            token: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    pub fn set_token(&self, token: String) {
        *self.token.write() = Some(token);
    }

    pub fn clear_token(&self) {
        *self.token.write() = None;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().clone()
    }

    pub fn config(&self) -> &MacroApiConfig {
        &self.config
    }

    pub async fn upload_static_file(
        &self,
        file_name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<StaticFileUpload> {
        let response: StaticFilePresignResponse = self
            .put(
                &self.config.static_files_base_url,
                "/api/file",
                &serde_json::json!({
                    "file_name": file_name,
                    "content_type": content_type,
                }),
            )
            .await?;
        let request = self
            .http
            .put(&response.upload_url)
            .header("content-type", content_type)
            .body(bytes);
        self.execute_empty_ok(request)
            .await
            .context("static-file blob upload failed")?;
        Ok(StaticFileUpload {
            id: response.id,
            file_location: response.file_location,
        })
    }

    async fn execute_text_response(request: RequestBuilder) -> Result<HttpTextResponse> {
        reqwest_client::runtime()
            .spawn(async move {
                let response = request.send().await?;
                let status = response.status().as_u16();
                let request_id = response
                    .headers()
                    .get("x-request-id")
                    .or_else(|| response.headers().get("x-amzn-requestid"))
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let text = response.text().await?;
                Ok::<_, reqwest::Error>(HttpTextResponse {
                    status,
                    text,
                    request_id,
                    retry_after,
                })
            })
            .await
            .context("HTTP runtime task failed")?
            .context("request failed")
    }

    async fn execute<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let response = Self::execute_text_response(request).await?;

        if !(200..300).contains(&response.status) {
            return Err(macro_http_error(response).into());
        }
        serde_json::from_str(&response.text).context("failed to parse response JSON")
    }

    async fn execute_optional<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<Option<T>> {
        let response = Self::execute_text_response(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(macro_http_error(response).into());
        }
        if response.status == 204 || response.text.trim().is_empty() {
            return Ok(None);
        }
        serde_json::from_str(&response.text)
            .context("failed to parse response JSON")
            .map(Some)
    }

    async fn get<T: DeserializeOwned>(&self, base_url: &str, path: &str) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn get_optional<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
    ) -> Result<Option<T>> {
        let url = format!("{}{}", base_url, path);
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_optional(request).await
    }

    async fn get_with_bearer<T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        bearer_token: &str,
    ) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let request = self.http.get(url).bearer_auth(bearer_token);
        self.execute(request).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn post_empty<B: Serialize>(&self, base_url: &str, path: &str, body: &B) -> Result<()> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .post(&url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    async fn post_no_body_empty(&self, base_url: &str, path: &str) -> Result<()> {
        let url = format!("{}{}", base_url, path);
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    async fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .patch(&url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .put(&url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn put_empty<B: Serialize>(&self, base_url: &str, path: &str, body: &B) -> Result<()> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .put(&url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    async fn delete<T: DeserializeOwned>(&self, base_url: &str, path: &str) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn delete_with_body<B: Serialize, T: DeserializeOwned>(
        &self,
        base_url: &str,
        path: &str,
        body: &B,
    ) -> Result<T> {
        let url = format!("{}{}", base_url, path);
        let json_body = serde_json::to_string(body).context("failed to serialize request body")?;
        let mut request = self
            .http
            .delete(url)
            .header("content-type", "application/json")
            .body(json_body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    async fn execute_empty_ok(&self, request: RequestBuilder) -> Result<()> {
        let response = Self::execute_text_response(request).await?;
        if !(200..300).contains(&response.status) {
            return Err(macro_http_error(response).into());
        }
        Ok(())
    }

    async fn execute_bytes(&self, request: RequestBuilder) -> Result<Vec<u8>> {
        let (status, bytes, request_id, retry_after) = reqwest_client::runtime()
            .spawn(async move {
                let response = request.send().await?;
                let status = response.status().as_u16();
                let request_id = response
                    .headers()
                    .get("x-request-id")
                    .or_else(|| response.headers().get("x-amzn-requestid"))
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|value| value.to_str().ok())
                    .map(ToOwned::to_owned);
                let bytes = response.bytes().await?;
                Ok::<_, reqwest::Error>((status, bytes, request_id, retry_after))
            })
            .await
            .context("HTTP runtime task failed")?
            .context("request failed")?;
        if !(200..300).contains(&status) {
            return Err(macro_http_error(HttpTextResponse {
                status,
                text: String::from_utf8_lossy(bytes.as_ref()).into_owned(),
                request_id,
                retry_after,
            })
            .into());
        }
        Ok(bytes.to_vec())
    }

    // === Auth Service ===

    pub async fn start_passwordless_login(
        &self,
        email: &str,
        redirect_uri: &str,
    ) -> Result<PasswordlessLoginResponse> {
        self.post(
            &self.config.auth_base_url,
            "/login/passwordless",
            &serde_json::json!({
                "email": email,
                "redirect_uri": redirect_uri,
            }),
        )
        .await
    }

    pub async fn complete_passwordless_login(
        &self,
        email: &str,
        code: &str,
    ) -> Result<UserTokensResponse> {
        let path = format!(
            "/oauth/passwordless/{}?email={}&disable_redirect=true",
            urlencoding::encode(code),
            urlencoding::encode(email),
        );
        self.get(&self.config.auth_base_url, &path).await
    }

    pub async fn redeem_session_code(&self, session_code: &str) -> Result<UserTokensResponse> {
        self.get(
            &self.config.auth_base_url,
            &format!("/session/login/{}", urlencoding::encode(session_code)),
        )
        .await
    }

    pub async fn exchange_macro_api_token(
        &self,
        access_token: &str,
    ) -> Result<MacroApiTokenResponse> {
        // Exchanging an auth-service access token must not implicitly change
        // the authenticated identity used by every other service client. A
        // passwordless, SSO, restore, or refresh request can finish after the
        // user has already selected another saved account. The application
        // activates the returned token only after its generation/account guard
        // confirms that the originating auth transition still owns the session.
        self.get_with_bearer(
            &self.config.auth_base_url,
            "/jwt/macro_api_token",
            access_token,
        )
        .await
    }

    pub async fn refresh_tokens(
        &self,
        access_token: &str,
        refresh_token: &str,
    ) -> Result<UserTokensResponse> {
        let request = self
            .http
            .post(format!("{}/jwt/refresh", self.config.auth_base_url))
            .bearer_auth(access_token)
            .header("x-macro-refresh-token", refresh_token);
        self.execute(request).await
    }

    pub async fn me(&self, access_token: &str) -> Result<UserInfo> {
        self.get_with_bearer(&self.config.auth_base_url, "/user/me", access_token)
            .await
    }

    pub async fn get_team(&self) -> Result<Option<TeamWithMembers>> {
        self.get_optional(&self.config.auth_base_url, "/team").await
    }

    pub async fn get_user_teams(&self) -> Result<Vec<Team>> {
        self.get(&self.config.auth_base_url, "/team/user").await
    }

    pub async fn create_team(&self, request: &CreateTeamRequest) -> Result<Team> {
        self.post(&self.config.auth_base_url, "/team", request)
            .await
    }

    pub async fn patch_team(&self, request: &PatchTeamRequest) -> Result<()> {
        let url = format!("{}/team", self.config.auth_base_url);
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn delete_team(&self) -> Result<()> {
        let url = format!("{}/team", self.config.auth_base_url);
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn invite_to_team(&self, request: &InviteToTeamRequest) -> Result<()> {
        let url = format!("{}/team/invite", self.config.auth_base_url);
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn get_team_invites(&self) -> Result<TeamInvitesResponse> {
        self.get(&self.config.auth_base_url, "/team/invites").await
    }

    pub async fn get_user_invites(&self) -> Result<TeamInvitesResponse> {
        self.get(&self.config.auth_base_url, "/team/user/invites")
            .await
    }

    pub async fn delete_team_invite(&self, invite_id: &str) -> Result<()> {
        let url = format!(
            "{}/team/invite/{}",
            self.config.auth_base_url,
            urlencoding::encode(invite_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn accept_team_invite(&self, invite_id: &str) -> Result<()> {
        let url = format!(
            "{}/team/join/{}",
            self.config.auth_base_url,
            urlencoding::encode(invite_id)
        );
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn reject_team_invite(&self, invite_id: &str) -> Result<()> {
        let url = format!(
            "{}/team/join/{}",
            self.config.auth_base_url,
            urlencoding::encode(invite_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn remove_team_member(&self, user_id: &str) -> Result<()> {
        let url = format!(
            "{}/team/remove/{}",
            self.config.auth_base_url,
            urlencoding::encode(user_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn toggle_team_auto_join_domain(&self) -> Result<serde_json::Value> {
        self.post(
            &self.config.auth_base_url,
            "/team/auto-join-domain/toggle",
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn toggle_team_non_admin_invites(&self) -> Result<serde_json::Value> {
        self.post(
            &self.config.auth_base_url,
            "/team/non-admin-invites/toggle",
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn get_user_name(&self) -> Result<UserName> {
        self.get(&self.config.auth_base_url, "/user/name").await
    }

    pub async fn get_profile_pictures(&self, user_ids: Vec<String>) -> Result<ProfilePictures> {
        self.post(
            &self.config.auth_base_url,
            "/user/profile_pictures",
            &GetProfilePicturesRequestBody {
                user_id_list: user_ids,
            },
        )
        .await
    }

    /// Sets or clears the current user's profile picture. Passing an empty URL
    /// has the auth-service's documented remove semantics.
    pub async fn put_profile_picture(&self, url: &str) -> Result<()> {
        let url = format!(
            "{}/user/profile_picture?url={}",
            self.config.auth_base_url,
            urlencoding::encode(url)
        );
        let mut request = self.http.put(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn get_user_names(&self, user_ids: Vec<String>) -> Result<UserNamesResponse> {
        self.post(
            &self.config.auth_base_url,
            "/user/get_names",
            &UserIdsRequest { user_ids },
        )
        .await
    }

    pub async fn get_user_names_with_email(
        &self,
        user_ids: Vec<String>,
    ) -> Result<UserNamesResponse> {
        self.post(
            &self.config.auth_base_url,
            "/user/get_names_with_email",
            &UserIdsRequest { user_ids },
        )
        .await
    }

    pub async fn list_contacts(&self) -> Result<GetContactsResponse> {
        self.get(&self.config.contacts_base_url, "/contacts").await
    }

    pub async fn add_contact(&self, user_id: String) -> Result<()> {
        let url = format!("{}/contacts", self.config.contacts_base_url);
        let body = serde_json::to_string(&AddContactRequest { user_id })?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn put_user_name(
        &self,
        first_name: Option<&str>,
        last_name: Option<&str>,
    ) -> Result<()> {
        let mut path = String::from("/user/name?");
        if let Some(first_name) = first_name {
            path.push_str("first_name=");
            path.push_str(&urlencoding::encode(first_name));
            path.push('&');
        }
        if let Some(last_name) = last_name {
            path.push_str("last_name=");
            path.push_str(&urlencoding::encode(last_name));
        }
        let url = format!(
            "{}{}",
            self.config.auth_base_url,
            path.trim_end_matches('&')
        );
        let mut request = self.http.put(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn patch_user_onboarding(&self, request: &PatchUserOnboardingRequest) -> Result<()> {
        let url = format!("{}/user/onboarding", self.config.auth_base_url);
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn get_user_quota(&self) -> Result<Option<UserQuota>> {
        self.get_optional(&self.config.auth_base_url, "/user/quota")
            .await
    }

    pub async fn create_checkout_session(
        &self,
        request: &CreateCheckoutSessionRequest,
    ) -> Result<StripeSessionResponse> {
        self.post(
            &self.config.auth_base_url,
            "/user/stripe/checkoutv2",
            request,
        )
        .await
    }

    pub async fn create_portal_session(
        &self,
        request: &CreatePortalSessionRequest,
    ) -> Result<StripeSessionResponse> {
        self.post(&self.config.auth_base_url, "/user/stripe/portal", request)
            .await
    }

    pub async fn github_connection_state(&self) -> Result<GithubConnectionState> {
        let url = format!("{}/link/github/status", self.config.auth_base_url);
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        let (status, text) = reqwest_client::runtime()
            .spawn(async move {
                let response = request.send().await?;
                let status = response.status();
                let text = response.text().await?;
                Ok::<_, reqwest::Error>((status, text))
            })
            .await
            .context("HTTP runtime task failed")?
            .context("request failed")?;
        match status.as_u16() {
            404 => Ok(GithubConnectionState::Disconnected),
            428 => Ok(GithubConnectionState::ReauthenticationRequired),
            _ if status.is_success() => {
                let response: GithubLinkStatusResponse =
                    serde_json::from_str(&text).context("failed to parse GitHub link status")?;
                Ok(if response.reauthentication_required {
                    GithubConnectionState::ReauthenticationRequired
                } else {
                    GithubConnectionState::Connected
                })
            }
            _ => anyhow::bail!("HTTP {}: {}", status, text),
        }
    }

    pub async fn init_github_link(
        &self,
        original_url: Option<&str>,
    ) -> Result<InitGithubLinkResponse> {
        let path = original_url.map_or_else(
            || "/link/github".to_owned(),
            |url| {
                format!(
                    "/link/github?original_url={}",
                    urlencoding::encode(&urlencoding::encode(url))
                )
            },
        );
        self.post(&self.config.auth_base_url, &path, &serde_json::json!({}))
            .await
    }

    pub async fn delete_github_link(&self) -> Result<()> {
        let url = format!("{}/link/github", self.config.auth_base_url);
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub fn github_install_url(&self) -> String {
        format!("{}/github/install-sync", self.config.storage_base_url)
    }

    /// Returns Macro's stable task branch name. GitHub's server-side app uses
    /// this identifier to associate branches and pull requests with the task
    /// and to drive the documented bidirectional status transitions.
    pub async fn get_document_branch_name(&self, document_id: &str) -> Result<BranchNameResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/documents/{}/branch_name",
                urlencoding::encode(document_id)
            ),
        )
        .await
    }

    /// Loads the pull requests associated with a task from storage and then
    /// asks auth's authenticated GitHub proxy for fresh lifecycle/check data.
    /// The stored response remains usable when enrichment is unavailable.
    pub async fn get_document_github_pull_requests(
        &self,
        document_id: &str,
    ) -> Result<Vec<GithubPullRequest>> {
        let stored: GithubPullRequestsResponse = self
            .get(
                &self.config.storage_base_url,
                &format!("/documents/{}/github_prs", urlencoding::encode(document_id)),
            )
            .await?;
        if stored.pull_requests.is_empty() {
            return Ok(Vec::new());
        }
        let request = EnrichGithubPullRequestsRequest {
            pull_requests: stored
                .pull_requests
                .iter()
                .map(|pull_request| GithubPullRequestRef {
                    display_name: pull_request.display_name.clone(),
                    github_key: pull_request.github_key.clone(),
                    number: pull_request.number,
                    owner: pull_request.owner.clone(),
                    repo: pull_request.repo.clone(),
                    url: pull_request.url.clone(),
                })
                .collect(),
        };
        let enriched: Result<GithubPullRequestsResponse> = self
            .post(
                &self.config.auth_base_url,
                "/github_pull_requests/enrich",
                &request,
            )
            .await;
        Ok(match enriched {
            Ok(enriched) => {
                merge_github_pull_requests(stored.pull_requests, enriched.pull_requests)
            }
            Err(_) => stored.pull_requests,
        })
    }

    pub async fn get_cursor_api_key_status(&self) -> Result<CursorApiKeyStatus> {
        self.get(&self.config.auth_base_url, "/cursor-api-key")
            .await
    }

    pub async fn put_cursor_api_key(&self, api_key: String) -> Result<CursorApiKeyStatus> {
        self.put(
            &self.config.auth_base_url,
            "/cursor-api-key",
            &PutCursorApiKeyRequest { api_key },
        )
        .await
    }

    pub async fn delete_cursor_api_key(&self) -> Result<CursorApiKeyStatus> {
        self.delete(&self.config.auth_base_url, "/cursor-api-key")
            .await
    }

    pub async fn list_cursor_models(&self) -> Result<CursorModelsResponse> {
        self.get(&self.config.auth_base_url, "/cursor-api-key/models")
            .await
    }

    pub async fn put_cursor_default_model(
        &self,
        model_id: Option<String>,
    ) -> Result<CursorApiKeyStatus> {
        self.put(
            &self.config.auth_base_url,
            "/cursor-api-key/default-model",
            &PutCursorDefaultModelRequest { model_id },
        )
        .await
    }

    pub async fn delete_user(&self) -> Result<()> {
        let url = format!("{}/user/me", self.config.auth_base_url);
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn logout(&self) -> Result<serde_json::Value> {
        let response = self
            .post(
                &self.config.auth_base_url,
                "/logout",
                &serde_json::json!({}),
            )
            .await?;
        *self.token.write() = None;
        Ok(response)
    }

    // === Document Storage Service ===

    pub async fn list_projects(&self) -> Result<Vec<StorageProject>> {
        let response: ProjectsResponse =
            self.get(&self.config.storage_base_url, "/projects").await?;
        Ok(response.data)
    }

    pub async fn create_project(&self, request: &CreateProjectRequest) -> Result<StorageProject> {
        let response: CreateProjectResponse = self
            .post(&self.config.storage_base_url, "/projects", request)
            .await?;
        Ok(response.data)
    }

    pub async fn get_project_content(&self, project_id: &str) -> Result<ProjectContentResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!("/projects/{}/content", urlencoding::encode(project_id)),
        )
        .await
    }

    pub async fn edit_project(
        &self,
        project_id: &str,
        request: &PatchProjectRequest,
    ) -> Result<()> {
        let url = format!(
            "{}/v2/projects/{}",
            self.config.storage_base_url,
            urlencoding::encode(project_id)
        );
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn delete_project(&self, project_id: &str) -> Result<()> {
        let url = format!(
            "{}/projects/{}",
            self.config.storage_base_url,
            urlencoding::encode(project_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn restore_project(&self, project_id: &str) -> Result<()> {
        let url = format!(
            "{}/projects/{}/revert_delete",
            self.config.storage_base_url,
            urlencoding::encode(project_id)
        );
        let mut request = self.http.put(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn permanently_delete_project(&self, project_id: &str) -> Result<()> {
        let url = format!(
            "{}/projects/{}/permanent",
            self.config.storage_base_url,
            urlencoding::encode(project_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_soup_items(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!(
            "/items/soup?limit={limit}&expand=true&sort_method=viewed_updated&sort_direction=desc"
        );
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.get(&self.config.storage_base_url, &path).await
    }

    /// Runs the AST soup endpoint used by Macro's entity-scoped views.
    ///
    /// The filter AST remains JSON because its recursive, tagged union is
    /// intentionally shared with the open-source backend and evolves often.
    pub async fn list_soup_ast_items(
        &self,
        request: &serde_json::Value,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = "/items/soup/ast".to_owned();
        if let Some(cursor) = cursor {
            path.push_str("?cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.post(&self.config.storage_base_url, &path, request)
            .await
    }

    /// Loads the first independently paginated page for every matching Soup
    /// group. The response ordering and labels are owned by the backend.
    pub async fn list_grouped_soup_ast_items(
        &self,
        filters: &serde_json::Value,
        group_by: &SoupGroupByField,
        sort_method: GroupedSoupSort,
        per_group_limit: u16,
    ) -> Result<GroupedSoupPage> {
        let mut request = grouped_soup_filter_object(filters)?;
        request.insert("mode".into(), serde_json::json!("initial"));
        request.insert("group_by".into(), serde_json::to_value(group_by)?);
        request.insert("sort_method".into(), serde_json::to_value(sort_method)?);
        request.insert("per_group_limit".into(), serde_json::json!(per_group_limit));
        self.post(
            &self.config.storage_base_url,
            "/items/soup/ast/grouped",
            &serde_json::Value::Object(request),
        )
        .await
    }

    /// Loads the next page for one group without disturbing any sibling
    /// groups. `cursor` is the opaque cursor returned in that group's metadata.
    pub async fn list_grouped_soup_ast_group_page(
        &self,
        filters: &serde_json::Value,
        group_by: &SoupGroupByField,
        group_key: &str,
        sort_method: GroupedSoupSort,
        limit: u16,
        cursor: &str,
    ) -> Result<GroupedSoupPage> {
        let mut request = grouped_soup_filter_object(filters)?;
        request.insert("mode".into(), serde_json::json!("group_page"));
        request.insert("group_by".into(), serde_json::to_value(group_by)?);
        request.insert("group_key".into(), serde_json::json!(group_key));
        request.insert("sort_method".into(), serde_json::to_value(sort_method)?);
        request.insert("limit".into(), serde_json::json!(limit));
        let path = format!(
            "/items/soup/ast/grouped?cursor={}",
            urlencoding::encode(cursor)
        );
        self.post(
            &self.config.storage_base_url,
            &path,
            &serde_json::Value::Object(request),
        )
        .await
    }

    pub async fn create_task(
        &self,
        name: &str,
        markdown: Option<&str>,
        project_id: Option<&str>,
        team_id: Option<&str>,
        property_values: Option<&[TaskPropertyInput]>,
        share_with_team: bool,
    ) -> Result<CreateTaskResponse> {
        self.post(
            &self.config.storage_base_url,
            "/documents/create_task",
            &CreateTaskRequest {
                task_name: name,
                project_id,
                team_id,
                markdown,
                property_values,
                share_with_team,
            },
        )
        .await
    }

    pub async fn create_snippet(
        &self,
        name: String,
        markdown: Option<String>,
        project_id: Option<String>,
    ) -> Result<CreateSnippetResponse> {
        self.post(
            &self.config.storage_base_url,
            "/documents/create_snippet",
            &CreateSnippetRequest {
                markdown,
                project_id,
                snippet_name: name,
            },
        )
        .await
    }

    pub async fn create_markdown(
        &self,
        name: &str,
        markdown: &str,
        project_id: Option<&str>,
    ) -> Result<CreateMarkdownResponse> {
        self.post(
            &self.config.storage_base_url,
            "/documents/create_markdown",
            &serde_json::json!({
                "documentName": name,
                "markdown": markdown,
                "projectId": project_id,
            }),
        )
        .await
    }

    /// Creates a DSS document, uploads its bytes to the returned S3 URL with
    /// Macro's SHA-256 checksum header, and rolls back the metadata record if
    /// the upload fails.
    pub async fn upload_document_bytes(
        &self,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        project_id: Option<String>,
    ) -> Result<UploadedDocument> {
        self.upload_document_bytes_with_progress(
            file_name,
            mime_type,
            bytes,
            project_id,
            UploadControl::default(),
            None,
        )
        .await
    }

    pub async fn upload_document_bytes_with_progress(
        &self,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
        project_id: Option<String>,
        control: UploadControl,
        progress: Option<futures::channel::mpsc::UnboundedSender<UploadProgress>>,
    ) -> Result<UploadedDocument> {
        let file_type = std::path::Path::new(file_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        self.upload_typed_document_bytes_with_progress(
            file_name,
            file_type.as_deref(),
            mime_type,
            bytes,
            project_id,
            control,
            progress,
        )
        .await
    }

    /// Streams a local file to Macro's presigned document upload without ever
    /// materializing the complete file in memory. A first bounded pass computes
    /// the service-required SHA-256; the second pass emits fixed-size chunks and
    /// remains cancellable between reads.
    pub async fn upload_document_path_with_progress(
        &self,
        file_name: &str,
        mime_type: &str,
        path: PathBuf,
        maximum_bytes: u64,
        project_id: Option<String>,
        control: UploadControl,
        progress: Option<futures::channel::mpsc::UnboundedSender<UploadProgress>>,
    ) -> Result<UploadedDocument> {
        let file_type = Path::new(file_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        let digest_path = path.clone();
        let digest_control = control.clone();
        let (sha, checksum, total_bytes) = reqwest_client::runtime()
            .spawn_blocking(move || {
                bounded_file_digest(&digest_path, maximum_bytes, &digest_control)
            })
            .await
            .context("file checksum task failed")??;
        if control.is_cancelled() {
            anyhow::bail!("upload cancelled");
        }
        let file = std::fs::File::open(&path)
            .with_context(|| format!("failed to reopen {} for upload", path.display()))?;
        let stream_control = control.clone();
        let stream_progress = progress.clone();
        let upload_stream = futures::stream::unfold((file, 0_u64), move |(mut file, offset)| {
            let control = stream_control.clone();
            let progress = stream_progress.clone();
            async move {
                if offset >= total_bytes || control.is_cancelled() {
                    return None;
                }
                let remaining = total_bytes - offset;
                let mut chunk = vec![0_u8; remaining.min(256 * 1024) as usize];
                match file.read(&mut chunk) {
                    Ok(0) => Some((
                        Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "upload source changed after checksum",
                        )),
                        (file, total_bytes),
                    )),
                    Ok(read) => {
                        chunk.truncate(read);
                        let uploaded_bytes = offset + read as u64;
                        if let Some(progress) = progress {
                            let _ = progress.unbounded_send(UploadProgress {
                                uploaded_bytes,
                                total_bytes,
                            });
                        }
                        Some((Ok::<Vec<u8>, std::io::Error>(chunk), (file, uploaded_bytes)))
                    }
                    Err(error) => Some((Err(error), (file, total_bytes))),
                }
            }
        });
        self.upload_typed_document_body_with_progress(
            file_name,
            file_type,
            mime_type,
            reqwest::Body::wrap_stream(upload_stream),
            sha,
            checksum,
            total_bytes,
            project_id,
            control,
            progress,
        )
        .await
    }

    /// Uploads a zip archive through Macro's folder-extraction pipeline. The
    /// returned request remains pending until connection-gateway emits the
    /// matching `bulk_upload` terminal status.
    pub async fn upload_zip_folder_bytes(
        &self,
        archive_name: &str,
        bytes: Vec<u8>,
        parent_id: Option<String>,
    ) -> Result<PendingFolderUpload> {
        let digest = Sha256::digest(&bytes);
        let sha = format!("{digest:x}");
        let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
        let name = std::path::Path::new(archive_name)
            .file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or(archive_name)
            .to_owned();
        let response: UploadExtractFolderResponse = self
            .post(
                &self.config.storage_base_url,
                "/projects/upload_extract",
                &UploadExtractFolderRequest {
                    name: Some(name.clone()),
                    parent_id,
                    sha,
                },
            )
            .await?;
        if response.error {
            anyhow::bail!("folder extraction request failed");
        }
        let request_id = response.data.request_id;
        let upload_url = response.data.presigned_url;
        let upload_result = reqwest_client::runtime()
            .spawn(async move {
                let response = Client::new()
                    .put(upload_url)
                    .header("content-type", "application/zip")
                    .header("x-amz-checksum-sha256", checksum)
                    .body(bytes)
                    .send()
                    .await?;
                let status = response.status();
                let text = response.text().await?;
                Ok::<_, reqwest::Error>((status, text))
            })
            .await
            .context("folder archive upload runtime failed")?
            .context("folder archive upload request failed")?;
        if !upload_result.0.is_success() {
            anyhow::bail!(
                "folder archive upload returned HTTP {}: {}",
                upload_result.0,
                upload_result.1
            );
        }
        Ok(PendingFolderUpload { name, request_id })
    }

    pub async fn upload_typed_document_bytes(
        &self,
        document_name: &str,
        file_type: Option<&str>,
        mime_type: &str,
        bytes: Vec<u8>,
        project_id: Option<String>,
    ) -> Result<UploadedDocument> {
        self.upload_typed_document_bytes_with_progress(
            document_name,
            file_type,
            mime_type,
            bytes,
            project_id,
            UploadControl::default(),
            None,
        )
        .await
    }

    pub async fn upload_typed_document_bytes_with_progress(
        &self,
        document_name: &str,
        file_type: Option<&str>,
        mime_type: &str,
        bytes: Vec<u8>,
        project_id: Option<String>,
        control: UploadControl,
        progress: Option<futures::channel::mpsc::UnboundedSender<UploadProgress>>,
    ) -> Result<UploadedDocument> {
        if control.is_cancelled() {
            anyhow::bail!("upload cancelled");
        }
        let digest = Sha256::digest(&bytes);
        let sha = format!("{digest:x}");
        let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
        let total_bytes = bytes.len() as u64;
        let stream_control = control.clone();
        let stream_progress = progress.clone();
        let upload_stream = futures::stream::unfold((bytes, 0_usize), move |(bytes, offset)| {
            let control = stream_control.clone();
            let progress = stream_progress.clone();
            async move {
                if offset >= bytes.len() || control.is_cancelled() {
                    return None;
                }
                let end = (offset + 256 * 1024).min(bytes.len());
                let chunk = bytes[offset..end].to_vec();
                if let Some(progress) = progress {
                    let _ = progress.unbounded_send(UploadProgress {
                        uploaded_bytes: end as u64,
                        total_bytes: bytes.len() as u64,
                    });
                }
                Some((Ok::<Vec<u8>, std::io::Error>(chunk), (bytes, end)))
            }
        });
        self.upload_typed_document_body_with_progress(
            document_name,
            file_type.map(ToOwned::to_owned),
            mime_type,
            reqwest::Body::wrap_stream(upload_stream),
            sha,
            checksum,
            total_bytes,
            project_id,
            control,
            progress,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn upload_typed_document_body_with_progress(
        &self,
        document_name: &str,
        file_type: Option<String>,
        mime_type: &str,
        body: reqwest::Body,
        sha: String,
        checksum: String,
        total_bytes: u64,
        project_id: Option<String>,
        control: UploadControl,
        progress: Option<futures::channel::mpsc::UnboundedSender<UploadProgress>>,
    ) -> Result<UploadedDocument> {
        let response: serde_json::Value = self
            .post(
                &self.config.storage_base_url,
                "/documents",
                &CreateBinaryDocumentRequest {
                    document_name: document_name.to_owned(),
                    sha,
                    file_type: file_type.clone(),
                    mime_type: Some(mime_type.to_owned()),
                    project_id,
                },
            )
            .await?;
        let data = response.get("data").unwrap_or(&response);
        let metadata = data
            .get("documentMetadata")
            .context("upload response omitted documentMetadata")?;
        let document_id = metadata
            .get("documentId")
            .and_then(serde_json::Value::as_str)
            .context("upload response omitted documentId")?
            .to_owned();
        let document_name = metadata
            .get("documentName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(document_name)
            .to_owned();
        let document_version_id = metadata
            .get("documentVersionId")
            .and_then(serde_json::Value::as_u64);
        let presigned_url = data
            .get("presignedUrl")
            .and_then(serde_json::Value::as_str)
            .context("upload response omitted presignedUrl")?
            .to_owned();
        let content_type = data
            .get("contentType")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(mime_type)
            .to_owned();
        if control.is_cancelled() {
            let _ = self.delete_document(&document_id).await;
            anyhow::bail!("upload cancelled");
        }
        if let Some(progress) = progress.as_ref() {
            let _ = progress.unbounded_send(UploadProgress {
                uploaded_bytes: 0,
                total_bytes,
            });
        }
        let upload_result: Result<_> = async {
            reqwest_client::runtime()
                .spawn(async move {
                    let response = Client::new()
                        .put(presigned_url)
                        .header("content-type", content_type)
                        .header("x-amz-checksum-sha256", checksum)
                        .header("content-length", total_bytes)
                        .body(body)
                        .send()
                        .await?;
                    let status = response.status();
                    let text = response.text().await?;
                    Ok::<_, reqwest::Error>((status, text))
                })
                .await
                .context("file upload runtime task failed")?
                .context("file upload request failed")
        }
        .await;
        let upload_result = match upload_result {
            Ok(upload_result) => upload_result,
            Err(error) => {
                let _ = self.delete_document(&document_id).await;
                if control.is_cancelled() {
                    anyhow::bail!("upload cancelled");
                }
                return Err(error);
            }
        };
        if control.is_cancelled() {
            let _ = self.delete_document(&document_id).await;
            anyhow::bail!("upload cancelled");
        }
        if !upload_result.0.is_success() {
            let _ = self.delete_document(&document_id).await;
            anyhow::bail!(
                "file upload returned HTTP {}: {}",
                upload_result.0,
                upload_result.1
            );
        }
        Ok(UploadedDocument {
            document_id,
            document_name,
            document_version_id,
            file_type,
        })
    }

    /// Fetches the generated storage-service V3 content lifecycle response.
    /// `document_version_id` is supplied after uploads so finalization for a
    /// newer revision cannot be confused with an already-ready older version.
    pub async fn get_document_location(
        &self,
        document_id: &str,
        document_version_id: Option<u64>,
    ) -> Result<DocumentLocationResponse> {
        let mut path = format!(
            "/documents/{}/location_v3",
            urlencoding::encode(document_id)
        );
        if let Some(document_version_id) = document_version_id {
            path.push_str("?document_version_id=");
            path.push_str(&document_version_id.to_string());
        }
        self.get(&self.config.storage_base_url, &path).await
    }

    pub async fn rename_document(
        &self,
        document_id: &str,
        name: &str,
    ) -> Result<serde_json::Value> {
        self.edit_document(
            document_id,
            &EditDocumentRequest {
                document_name: Some(name.to_owned()),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn move_document(
        &self,
        document_id: &str,
        project_id: Option<String>,
    ) -> Result<serde_json::Value> {
        self.edit_document(
            document_id,
            &EditDocumentRequest {
                project_id: Some(project_id),
                ..Default::default()
            },
        )
        .await
    }

    pub async fn edit_document(
        &self,
        document_id: &str,
        request: &EditDocumentRequest,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/documents/{}", urlencoding::encode(document_id)),
            request,
        )
        .await
    }

    pub async fn copy_document(
        &self,
        document_id: &str,
        new_name: &str,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/documents/{}/copy", urlencoding::encode(document_id)),
            &CopyDocumentRequest {
                document_name: new_name.to_owned(),
                version_id: None,
            },
        )
        .await
    }

    /// Copies the exact causal version represented by a Loro history frontier.
    /// This preserves the complete document schema and opaque nodes; exporting
    /// a Markdown projection and creating a new document would be lossy.
    pub async fn copy_document_at_sync_version(
        &self,
        document_id: &str,
        new_name: &str,
        version: SyncServiceVersionId,
    ) -> Result<CopiedDocument> {
        let response: CopyDocumentResponseEnvelope = self
            .post(
                &self.config.storage_base_url,
                &format!("/documents/{}/copy", urlencoding::encode(document_id)),
                &CopyDocumentRequest {
                    document_name: new_name.to_owned(),
                    version_id: Some(version),
                },
            )
            .await?;
        Ok(response.data.document_metadata)
    }

    pub async fn delete_document(&self, document_id: &str) -> Result<serde_json::Value> {
        self.delete(
            &self.config.storage_base_url,
            &format!("/documents/{}", urlencoding::encode(document_id)),
        )
        .await
    }

    pub async fn restore_document(&self, document_id: &str) -> Result<serde_json::Value> {
        self.put(
            &self.config.storage_base_url,
            &format!(
                "/documents/{}/revert_delete",
                urlencoding::encode(document_id)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn permanently_delete_document(&self, document_id: &str) -> Result<()> {
        let url = format!(
            "{}/documents/{}/permanent",
            self.config.storage_base_url,
            urlencoding::encode(document_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_recently_deleted(&self) -> Result<serde_json::Value> {
        self.get(&self.config.storage_base_url, "/recents/deleted")
            .await
    }

    pub async fn restore_chat(&self, chat_id: &str) -> Result<()> {
        let url = format!(
            "{}/chats/{}/revert_delete",
            self.config.cognition_base_url,
            urlencoding::encode(chat_id)
        );
        let mut request = self.http.put(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn permanently_delete_chat(&self, chat_id: &str) -> Result<()> {
        let url = format!(
            "{}/chat/{}/permanent",
            self.config.cognition_base_url,
            urlencoding::encode(chat_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn document_export_url(&self, document_id: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct ExportResponse {
            presigned_url: String,
        }
        self.get::<ExportResponse>(
            &self.config.storage_base_url,
            &format!("/documents/{}/export", urlencoding::encode(document_id)),
        )
        .await
        .map(|response| response.presigned_url)
    }

    pub async fn get_document_team_share(&self, document_id: &str) -> Result<serde_json::Value> {
        self.get(
            &self.config.storage_base_url,
            &format!("/documents/{}/team_share", urlencoding::encode(document_id)),
        )
        .await
    }

    pub async fn set_document_team_share(
        &self,
        document_id: &str,
        share_with_team: bool,
    ) -> Result<serde_json::Value> {
        self.put(
            &self.config.storage_base_url,
            &format!("/documents/{}/team_share", urlencoding::encode(document_id)),
            &SetDocumentTeamShareRequest { share_with_team },
        )
        .await
    }

    pub async fn get_document_content(&self, document_id: &str) -> Result<String> {
        let bytes = self.download_document_bytes(document_id).await?;
        String::from_utf8(bytes).context("document content is not valid UTF-8")
    }

    pub async fn download_document_bytes(&self, document_id: &str) -> Result<Vec<u8>> {
        #[derive(Deserialize)]
        struct ExportResponse {
            presigned_url: String,
        }

        let export: ExportResponse = self
            .get(
                &self.config.storage_base_url,
                &format!("/documents/{}/export", urlencoding::encode(document_id)),
            )
            .await?;
        let url = export.presigned_url;
        let (status, bytes) = reqwest_client::runtime()
            .spawn(async move {
                let response = Client::new().get(url).send().await?;
                let status = response.status();
                let bytes = response.bytes().await?.to_vec();
                Ok::<_, reqwest::Error>((status, bytes))
            })
            .await
            .context("document download task failed")?
            .context("failed to download document content")?;
        if !status.is_success() {
            anyhow::bail!("document download returned HTTP {status}");
        }
        Ok(bytes)
    }

    /// Mint the short-lived capability token required by Macro's sync worker.
    /// The user's API token authenticates this storage-service request; the
    /// returned token is scoped to a single collaboration surface.
    pub async fn get_document_permissions_token(
        &self,
        document_id: &str,
    ) -> Result<DocumentPermissionsTokenResponse> {
        let url = format!(
            "{}/documents/permissions_token/{}",
            self.config.storage_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id)
        );
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute(request).await
    }

    pub async fn get_sync_snapshot(
        &self,
        document_id: &str,
        permission_token: &str,
    ) -> Result<Vec<u8>> {
        let url = format!(
            "{}/document/{}/snapshot",
            self.config.sync_http_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id)
        );
        self.execute_bytes(
            self.http
                .get(url)
                .header("Origin", &self.config.web_app_origin)
                .header("Content-Type", "application/octet-stream")
                .bearer_auth(permission_token),
        )
        .await
    }

    pub async fn get_sync_raw_document(
        &self,
        document_id: &str,
        permission_token: &str,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/document/{}/raw",
            self.config.sync_http_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id)
        );
        self.execute(
            self.http
                .get(url)
                .header("Origin", &self.config.web_app_origin)
                .header("Content-Type", "application/json")
                .bearer_auth(permission_token),
        )
        .await
    }

    pub async fn get_sync_document_metadata(
        &self,
        document_id: &str,
        permission_token: &str,
    ) -> Result<SyncDocumentMetadata> {
        let url = format!(
            "{}/document/{}/metadata",
            self.config.sync_http_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id)
        );
        self.execute(
            self.http
                .get(url)
                .header("Origin", &self.config.web_app_origin)
                .bearer_auth(permission_token),
        )
        .await
    }

    pub async fn get_sync_node_blame(
        &self,
        document_id: &str,
        node_id: &str,
        permission_token: &str,
    ) -> Result<SyncNodeBlame> {
        let url = format!(
            "{}/document/{}/blame/{}",
            self.config.sync_http_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id),
            urlencoding::encode(node_id)
        );
        self.execute(
            self.http
                .get(url)
                .header("Origin", &self.config.web_app_origin)
                .bearer_auth(permission_token),
        )
        .await
    }

    pub async fn simple_save_markdown(
        &self,
        document_id: &str,
        markdown: &str,
    ) -> Result<serde_json::Value> {
        self.simple_save_bytes(
            document_id,
            "document.md",
            "text/markdown",
            markdown.as_bytes().to_vec(),
        )
        .await
    }

    pub async fn simple_save_bytes(
        &self,
        document_id: &str,
        file_name: &str,
        mime_type: &str,
        bytes: Vec<u8>,
    ) -> Result<serde_json::Value> {
        let form = multipart::Form::new().part(
            "file",
            multipart::Part::bytes(bytes)
                .file_name(file_name.to_owned())
                .mime_str(mime_type)
                .context("failed to build simple-save upload")?,
        );
        let url = format!(
            "{}/documents/{}/simple_save",
            self.config.storage_base_url,
            urlencoding::encode(document_id)
        );
        let mut request = self.http.put(url).multipart(form);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute(request).await
    }

    pub async fn set_task_completed(&self, document_id: &str, completed: bool) -> Result<()> {
        const STATUS_PROPERTY_ID: &str = "00000001-0000-0000-0000-000000000002";
        const COMPLETED_STATUS_OPTION_ID: &str = "00000001-0000-0000-0002-000000000004";
        let path = format!(
            "/properties/entities/DOCUMENT/{}/{}/options/{}",
            urlencoding::encode(document_id),
            STATUS_PROPERTY_ID,
            COMPLETED_STATUS_OPTION_ID
        );
        let url = format!("{}{}", self.config.storage_base_url, path);
        let mut request = if completed {
            self.http.post(url)
        } else {
            self.http.delete(url)
        };
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn set_task_status(&self, document_id: &str, option_id: &str) -> Result<()> {
        const STATUS_PROPERTY_ID: &str = "00000001-0000-0000-0000-000000000002";
        let url = format!(
            "{}/properties/entities/DOCUMENT/{}/{}/options/{}",
            self.config.storage_base_url.trim_end_matches('/'),
            urlencoding::encode(document_id),
            STATUS_PROPERTY_ID,
            urlencoding::encode(option_id),
        );
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn get_task_duplicates(&self, document_id: &str) -> Result<Vec<TaskDuplicate>> {
        self.get::<TaskDuplicatesResponse>(
            &self.config.storage_base_url,
            &format!("/documents/{}/duplicates", urlencoding::encode(document_id)),
        )
        .await
        .map(|response| response.duplicates)
    }

    pub async fn search_similar_tasks(
        &self,
        task_name: &str,
        markdown: Option<&str>,
    ) -> Result<Vec<TaskSimilarityResult>> {
        self.post::<_, TaskSimilaritySearchResponse>(
            &self.config.storage_base_url,
            "/documents/similarity_search",
            &serde_json::json!({
                "taskName": task_name,
                "markdown": markdown,
            }),
        )
        .await
        .map(|response| response.results)
    }

    pub async fn dismiss_task_duplicates(
        &self,
        document_id: &str,
        match_ids: Vec<String>,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/documents/{}/duplicates/dismiss",
                urlencoding::encode(document_id)
            ),
            &DismissTaskDuplicatesRequest { match_ids },
        )
        .await
    }

    pub async fn delete_this_duplicate_task(
        &self,
        document_id: &str,
        match_id: &str,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/documents/{}/duplicates/{}/delete_this",
                urlencoding::encode(document_id),
                urlencoding::encode(match_id),
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn list_channels(
        &self,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut query = Vec::new();
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if let Some(cursor) = cursor {
            query.push(format!("cursor={}", urlencoding::encode(cursor)));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.get(
            &self.config.storage_base_url,
            &format!("/comms/channels{suffix}"),
        )
        .await
    }

    pub async fn create_channel(&self, request: &CreateChannelRequest) -> Result<ChannelCreated> {
        self.post(&self.config.storage_base_url, "/channels", request)
            .await
    }

    pub async fn get_or_create_direct_message(
        &self,
        request: &GetOrCreateDmRequest,
    ) -> Result<GetOrCreateChannelResponse> {
        self.post(
            &self.config.storage_base_url,
            "/channels/get_or_create_dm",
            request,
        )
        .await
    }

    pub async fn get_or_create_private_channel(
        &self,
        request: &GetOrCreatePrivateRequest,
    ) -> Result<GetOrCreateChannelResponse> {
        self.post(
            &self.config.storage_base_url,
            "/channels/get_or_create_private",
            request,
        )
        .await
    }

    pub async fn patch_channel(
        &self,
        channel_id: &str,
        request: &PatchChannelRequest,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/channels/{}", urlencoding::encode(channel_id)),
            request,
        )
        .await
    }

    pub async fn list_channel_participants(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelParticipant>> {
        self.get(
            &self.config.storage_base_url,
            &format!("/channels/{}/participants", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn add_channel_participants(
        &self,
        channel_id: &str,
        participants: Vec<String>,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/participants", urlencoding::encode(channel_id)),
            &ChannelParticipantsRequest { participants },
        )
        .await
    }

    pub async fn remove_channel_participants(
        &self,
        channel_id: &str,
        participants: Vec<String>,
    ) -> Result<serde_json::Value> {
        self.delete_with_body(
            &self.config.storage_base_url,
            &format!("/channels/{}/participants", urlencoding::encode(channel_id)),
            &ChannelParticipantsRequest { participants },
        )
        .await
    }

    pub async fn join_channel(&self, channel_id: &str) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/join", urlencoding::encode(channel_id)),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn get_channel_join_link(&self, channel_id: &str) -> Result<ChannelJoinCodeResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!("/channels/{}/join-link", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn join_channel_by_code(&self, join_code: &str) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/join/{}", urlencoding::encode(join_code)),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn leave_channel(&self, channel_id: &str) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/leave", urlencoding::encode(channel_id)),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn get_channel(&self, channel_id: &str) -> Result<serde_json::Value> {
        self.get(
            &self.config.storage_base_url,
            &format!("/channels/{}", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn get_attachment_references(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<GetAttachmentReferencesResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/channels/attachments/{}/{}/references",
                urlencoding::encode(entity_type),
                urlencoding::encode(entity_id)
            ),
        )
        .await
    }

    pub async fn create_entity_mention(
        &self,
        request: &CreateEntityMentionRequest,
    ) -> Result<CreateEntityMentionResponse> {
        self.post(&self.config.storage_base_url, "/channels/mentions", request)
            .await
    }

    pub async fn delete_entity_mention(
        &self,
        mention_id: &str,
    ) -> Result<DeleteEntityMentionResponse> {
        self.delete(
            &self.config.storage_base_url,
            &format!("/channels/mentions/{}", urlencoding::encode(mention_id)),
        )
        .await
    }

    pub async fn list_channel_messages(
        &self,
        channel_id: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value> {
        self.list_channel_messages_window(channel_id, limit, cursor, None, None)
            .await
    }

    pub async fn list_channel_messages_window(
        &self,
        channel_id: &str,
        limit: u32,
        cursor: Option<&str>,
        previous_cursor: Option<&str>,
        load_around_message_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let path = channel_messages_path(
            channel_id,
            limit,
            cursor,
            previous_cursor,
            load_around_message_id,
        );
        self.get(&self.config.storage_base_url, &path).await
    }

    pub async fn resolve_channel_message(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<ResolvedChannelMessage> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/channels/{}/messages/{}/resolve",
                urlencoding::encode(channel_id),
                urlencoding::encode(message_id)
            ),
        )
        .await
    }

    pub async fn send_channel_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> Result<ChannelMessageCreated> {
        self.post_channel_message(
            channel_id,
            &ChannelMessageRequest {
                attachments: Vec::new(),
                content: content.to_owned(),
                mentions: Vec::new(),
                nonce: None,
                thread_id: None,
            },
        )
        .await
    }

    pub async fn post_channel_message(
        &self,
        channel_id: &str,
        request: &ChannelMessageRequest,
    ) -> Result<ChannelMessageCreated> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/message", urlencoding::encode(channel_id)),
            request,
        )
        .await
    }

    /// Records the same channel view/interaction activity used by Macro web.
    /// This endpoint owns `viewed_at`, which is the durable unread boundary.
    pub async fn record_channel_activity(
        &self,
        channel_id: &str,
        activity_type: ChannelActivityType,
    ) -> Result<ChannelActivity> {
        self.post(
            &self.config.storage_base_url,
            "/channels/activity",
            &RecordChannelActivityRequest {
                channel_id: channel_id.to_owned(),
                activity_type,
            },
        )
        .await
    }

    pub async fn reply_to_channel_message(
        &self,
        channel_id: &str,
        thread_id: &str,
        content: &str,
    ) -> Result<ChannelMessageCreated> {
        self.post_channel_message(
            channel_id,
            &ChannelMessageRequest {
                attachments: Vec::new(),
                content: content.to_owned(),
                mentions: Vec::new(),
                nonce: None,
                thread_id: Some(thread_id.to_owned()),
            },
        )
        .await
    }

    pub async fn patch_channel_message(
        &self,
        channel_id: &str,
        message_id: &str,
        request: &PatchChannelMessageRequest,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!(
                "/channels/{}/message/{}",
                urlencoding::encode(channel_id),
                urlencoding::encode(message_id)
            ),
            request,
        )
        .await
    }

    pub async fn delete_channel_message(
        &self,
        channel_id: &str,
        message_id: &str,
        nonce: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut path = format!(
            "/channels/{}/message/{}",
            urlencoding::encode(channel_id),
            urlencoding::encode(message_id)
        );
        if let Some(nonce) = nonce {
            path.push_str("?nonce=");
            path.push_str(&urlencoding::encode(nonce));
        }
        self.delete(&self.config.storage_base_url, &path).await
    }

    pub async fn list_channel_thread_replies(
        &self,
        channel_id: &str,
        message_id: &str,
    ) -> Result<serde_json::Value> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/channels/{}/messages/{}/replies",
                urlencoding::encode(channel_id),
                urlencoding::encode(message_id)
            ),
        )
        .await
    }

    pub async fn mutate_channel_reaction(
        &self,
        channel_id: &str,
        request: &ChannelReactionRequest,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/reaction", urlencoding::encode(channel_id)),
            request,
        )
        .await
    }

    pub async fn update_channel_typing(
        &self,
        channel_id: &str,
        request: &ChannelTypingRequest,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/typing", urlencoding::encode(channel_id)),
            request,
        )
        .await
    }

    pub async fn get_document(&self, doc_id: &str) -> Result<serde_json::Value> {
        self.get(
            &self.config.storage_base_url,
            &format!("/documents/{}", urlencoding::encode(doc_id)),
        )
        .await
    }

    pub async fn get_starter_documents(&self) -> Result<StarterDocumentsResponse> {
        self.get(&self.config.storage_base_url, "/documents/starter_docs")
            .await
    }

    pub async fn list_projects_raw(&self) -> Result<serde_json::Value> {
        self.get(&self.config.storage_base_url, "/projects").await
    }

    pub async fn list_favorites(&self) -> Result<serde_json::Value> {
        self.get(&self.config.storage_base_url, "/favorites").await
    }

    pub async fn get_entity_permission(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<GetEntityPermissionResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/entity/{}/{}/permissions",
                urlencoding::encode(entity_type),
                urlencoding::encode(entity_id)
            ),
        )
        .await
    }

    pub async fn get_document_share_permissions(
        &self,
        document_id: &str,
    ) -> Result<SharePermissionV2> {
        self.get::<GetDocumentPermissionsResponseV2>(
            &self.config.storage_base_url,
            &format!(
                "/v2/documents/{}/permissions",
                urlencoding::encode(document_id)
            ),
        )
        .await
        .map(|response| response.document_permissions)
    }

    pub async fn get_project_share_permissions(
        &self,
        project_id: &str,
    ) -> Result<SharePermissionV2> {
        self.get(
            &self.config.storage_base_url,
            &format!("/projects/{}/permissions", urlencoding::encode(project_id)),
        )
        .await
    }

    pub async fn update_document_share_permissions(
        &self,
        document_id: &str,
        permissions: &UpdateSharePermissionRequestV2,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/documents/{}", urlencoding::encode(document_id)),
            &EditDocumentShareRequest {
                share_permission: permissions,
            },
        )
        .await
    }

    pub async fn update_project_share_permissions(
        &self,
        project_id: &str,
        permissions: &UpdateSharePermissionRequestV2,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/v2/projects/{}", urlencoding::encode(project_id)),
            &EditProjectShareRequest {
                share_permission: permissions,
            },
        )
        .await
    }

    pub async fn update_thread_share_permissions(
        &self,
        thread_id: &str,
        permissions: &UpdateSharePermissionRequestV2,
    ) -> Result<serde_json::Value> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/threads/{}", urlencoding::encode(thread_id)),
            &EditThreadShareRequest {
                share_permission: permissions,
            },
        )
        .await
    }

    pub async fn get_entity_properties(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/properties/entities/{}/{}?include_metadata=true",
                urlencoding::encode(entity_type),
                urlencoding::encode(entity_id)
            ),
        )
        .await
    }

    pub async fn list_property_definitions(
        &self,
        scope: &str,
        include_options: bool,
        for_entity_type: Option<&str>,
    ) -> Result<Vec<PropertyDefinitionResponse>> {
        let mut path = format!(
            "/properties/definitions?scope={}&include_options={include_options}",
            urlencoding::encode(scope)
        );
        if let Some(entity_type) = for_entity_type {
            path.push_str("&for_entity_type=");
            path.push_str(&urlencoding::encode(entity_type));
        }
        self.get(&self.config.storage_base_url, &path).await
    }

    pub async fn create_property_definition(
        &self,
        request: &CreatePropertyDefinitionRequest,
    ) -> Result<PropertyDefinition> {
        self.post(
            &self.config.storage_base_url,
            "/properties/definitions",
            request,
        )
        .await
    }

    pub async fn delete_property_definition(&self, definition_id: &str) -> Result<()> {
        let url = format!(
            "{}/properties/definitions/{}",
            self.config.storage_base_url,
            urlencoding::encode(definition_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn set_entity_property(
        &self,
        entity_type: &str,
        entity_id: &str,
        property_id: &str,
        value: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.put(
            &self.config.storage_base_url,
            &format!(
                "/properties/entities/{}/{}/{}",
                urlencoding::encode(entity_type),
                urlencoding::encode(entity_id),
                urlencoding::encode(property_id)
            ),
            &serde_json::json!({"value": value}),
        )
        .await
    }

    pub async fn delete_entity_property(&self, entity_property_id: &str) -> Result<()> {
        let url = format!(
            "{}/properties/entity_properties/{}",
            self.config.storage_base_url,
            urlencoding::encode(entity_property_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn add_entity_property_option(
        &self,
        entity_type: &str,
        entity_id: &str,
        property_id: &str,
        option_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/properties/entities/{}/{}/{}/options/{}",
            self.config.storage_base_url,
            urlencoding::encode(entity_type),
            urlencoding::encode(entity_id),
            urlencoding::encode(property_id),
            urlencoding::encode(option_id),
        );
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn remove_entity_property_option(
        &self,
        entity_type: &str,
        entity_id: &str,
        property_id: &str,
        option_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/properties/entities/{}/{}/{}/options/{}",
            self.config.storage_base_url,
            urlencoding::encode(entity_type),
            urlencoding::encode(entity_id),
            urlencoding::encode(property_id),
            urlencoding::encode(option_id),
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    /// Applies one shared multi-select option delta to many entities. Macro's
    /// properties service returns a result per entity so callers can reconcile
    /// successful edits while keeping permission/conflict failures actionable.
    pub async fn bulk_update_entities_property_options(
        &self,
        request: &BulkUpdateEntitiesPropertyOptionsRequest,
    ) -> Result<BulkUpdateEntitiesPropertyOptionsResponse> {
        self.post(
            &self.config.storage_base_url,
            "/properties/options/bulk",
            request,
        )
        .await
    }

    pub async fn list_tags(&self) -> Result<Vec<TagSetResponse>> {
        self.get(&self.config.storage_base_url, "/properties/tags")
            .await
    }

    pub async fn ensure_tag_set(&self, scope: &str) -> Result<TagSetResponse> {
        self.post(
            &self.config.storage_base_url,
            "/properties/tags",
            &EnsureTagSetRequest {
                scope: scope.to_owned(),
            },
        )
        .await
    }

    pub async fn create_tag(
        &self,
        scope: &str,
        value: &str,
        color: Option<String>,
    ) -> Result<TagOption> {
        let tag_set = self.ensure_tag_set(scope).await?;
        let definition = tag_set
            .definition
            .context("Macro tag set was provisioned without a definition")?;
        let display_order = tag_set
            .options
            .iter()
            .map(|option| option.display_order)
            .max()
            .unwrap_or(-1)
            + 1;
        self.create_string_property_option(&definition.id, value, color, display_order)
            .await
    }

    pub async fn create_string_property_option(
        &self,
        property_definition_id: &str,
        value: &str,
        color: Option<String>,
        display_order: i64,
    ) -> Result<TagOption> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/properties/definitions/{}/options",
                urlencoding::encode(property_definition_id)
            ),
            &AddStringPropertyOptionRequest {
                option_type: "select_string".into(),
                option: AddStringPropertyOption {
                    value: value.to_owned(),
                    color,
                    display_order,
                },
            },
        )
        .await
    }

    pub async fn update_property_option(
        &self,
        property_definition_id: &str,
        option_id: &str,
        request: &UpdatePropertyOptionRequest,
    ) -> Result<TagOption> {
        self.patch(
            &self.config.storage_base_url,
            &format!(
                "/properties/definitions/{}/options/{}",
                urlencoding::encode(property_definition_id),
                urlencoding::encode(option_id)
            ),
            request,
        )
        .await
    }

    pub async fn delete_property_option(
        &self,
        property_definition_id: &str,
        option_id: &str,
    ) -> Result<()> {
        let url = format!(
            "{}/properties/definitions/{}/options/{}",
            self.config.storage_base_url,
            urlencoding::encode(property_definition_id),
            urlencoding::encode(option_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn update_tag(
        &self,
        property_definition_id: &str,
        option_id: &str,
        request: &UpdatePropertyOptionRequest,
    ) -> Result<TagOption> {
        self.update_property_option(property_definition_id, option_id, request)
            .await
    }

    pub async fn delete_tag(&self, property_definition_id: &str, option_id: &str) -> Result<()> {
        self.delete_property_option(property_definition_id, option_id)
            .await
    }

    pub async fn get_document_comments(
        &self,
        document_id: &str,
    ) -> Result<Vec<DocumentCommentThread>> {
        self.get::<DocumentCommentsResponse>(
            &self.config.storage_base_url,
            &format!(
                "/annotations/comments/document/{}",
                urlencoding::encode(document_id)
            ),
        )
        .await
        .map(|response| response.data)
    }

    pub async fn create_document_comment(
        &self,
        document_id: &str,
        request: &CreateDocumentCommentRequest,
    ) -> Result<DocumentCommentThread> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/annotations/comments/document/{}",
                urlencoding::encode(document_id)
            ),
            request,
        )
        .await
    }

    pub async fn edit_document_comment(
        &self,
        comment_id: i64,
        request: &EditDocumentCommentRequest,
    ) -> Result<DocumentComment> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/annotations/comments/comment/{comment_id}"),
            request,
        )
        .await
    }

    pub async fn delete_document_comment(
        &self,
        comment_id: i64,
    ) -> Result<DeleteDocumentCommentResponse> {
        self.delete_with_body(
            &self.config.storage_base_url,
            &format!("/annotations/comments/comment/{comment_id}"),
            &DeleteDocumentCommentRequest {
                remove_anchor_thread_only: false,
            },
        )
        .await
    }

    pub async fn add_favorite(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value> {
        self.post(
            &self.config.storage_base_url,
            "/favorites",
            &serde_json::json!({
                "entityType": entity_type,
                "entityId": entity_id,
            }),
        )
        .await
    }

    pub async fn remove_favorite(
        &self,
        entity_type: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value> {
        self.delete(
            &self.config.storage_base_url,
            &format!(
                "/favorites/{}/{}",
                urlencoding::encode(entity_type),
                urlencoding::encode(entity_id)
            ),
        )
        .await
    }

    pub async fn list_reminders(
        &self,
        include_completed: bool,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<ReminderPage> {
        let mut path = format!("/reminders?includeCompleted={include_completed}&limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.get(&self.config.storage_base_url, &path).await
    }

    pub async fn create_reminder(&self, request: &CreateReminderRequest) -> Result<Reminder> {
        self.post(&self.config.storage_base_url, "/reminders", request)
            .await
    }

    pub async fn update_reminder(
        &self,
        reminder_id: &str,
        request: &UpdateReminderRequest,
    ) -> Result<Reminder> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/reminders/{}", urlencoding::encode(reminder_id)),
            request,
        )
        .await
    }

    pub async fn delete_reminder(&self, reminder_id: &str) -> Result<()> {
        let url = format!(
            "{}/reminders/{}",
            self.config.storage_base_url,
            urlencoding::encode(reminder_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_notifications(
        &self,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<NotificationPage> {
        let mut path = format!("/v1/user_notifications?limit={limit}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.get(&self.config.notification_base_url, &path).await
    }

    pub async fn get_notification_type_preferences(&self) -> Result<NotificationTypePreferences> {
        self.get(
            &self.config.notification_base_url,
            "/v1/user_notifications/preferences",
        )
        .await
    }

    pub async fn set_notification_type_enabled(
        &self,
        notification_event_type: &str,
        enabled: bool,
    ) -> Result<()> {
        let action = if enabled { "enable" } else { "disable" };
        let url = format!(
            "{}/v1/user_notifications/preferences/{}/{}",
            self.config.notification_base_url,
            urlencoding::encode(notification_event_type),
            action
        );
        let mut request = self.http.put(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_notification_unsubscribes(&self) -> Result<Vec<NotificationUnsubscribe>> {
        self.get(&self.config.notification_base_url, "/unsubscribe")
            .await
    }

    pub async fn set_notification_item_muted(
        &self,
        item_type: &str,
        item_id: &str,
        muted: bool,
    ) -> Result<()> {
        let url = format!(
            "{}/unsubscribe/item/{}/{}",
            self.config.notification_base_url,
            urlencoding::encode(item_type),
            urlencoding::encode(item_id)
        );
        let mut request = if muted {
            self.http.post(url)
        } else {
            self.http.delete(url)
        };
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn mark_notifications_seen(&self, ids: Vec<String>) -> Result<serde_json::Value> {
        self.patch(
            &self.config.notification_base_url,
            "/v2/user_notifications/bulk/seen",
            &NotificationBulkRequest {
                notification_ids: ids,
            },
        )
        .await
    }

    pub async fn mark_notifications_done(
        &self,
        ids: Vec<String>,
        done: bool,
    ) -> Result<serde_json::Value> {
        let path = if done {
            "/v2/user_notifications/bulk/done"
        } else {
            "/v2/user_notifications/bulk/undone"
        };
        self.patch(
            &self.config.notification_base_url,
            path,
            &NotificationBulkRequest {
                notification_ids: ids,
            },
        )
        .await
    }

    /// Updates every notification associated with canonical entities through
    /// Macro's GraphQL Soup mutation and returns the exact notification IDs
    /// required by the ID-scoped undo endpoint.
    pub async fn update_notifications_for_entities(
        &self,
        entities: Vec<NotificationEntityInput>,
        operation: NotificationUpdateOperation,
    ) -> Result<Vec<String>> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let response: GraphqlNotificationMutationResponse = self
            .post(
                &self.config.storage_base_url,
                "/items/soup/graphql",
                &serde_json::json!({
                    "operationName": "UpdateNotificationsForEntity",
                    "query": "mutation UpdateNotificationsForEntity($input: UpdateNotificationsForEntityInput!) { updateNotificationsForEntity(input: $input) { id } }",
                    "variables": {
                        "input": {
                            "entities": entities,
                            "operation": operation,
                        }
                    }
                }),
            )
            .await?;
        if !response.errors.is_empty() {
            anyhow::bail!(
                "GraphQL notification mutation failed: {}",
                response
                    .errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        let data = response
            .data
            .context("GraphQL notification mutation returned no data")?;
        Ok(data
            .update_notifications_for_entity
            .into_iter()
            .map(|notification| notification.id)
            .collect())
    }

    pub async fn delete_notification(&self, notification_id: &str) -> Result<serde_json::Value> {
        self.delete(
            &self.config.notification_base_url,
            &format!(
                "/v2/user_notifications/{}",
                urlencoding::encode(notification_id)
            ),
        )
        .await
    }

    pub async fn get_reminder(&self, reminder_id: &str) -> Result<Reminder> {
        self.get(
            &self.config.storage_base_url,
            &format!("/reminders/{}", urlencoding::encode(reminder_id)),
        )
        .await
    }

    pub async fn create_crm_company(
        &self,
        request: &CreateCrmCompanyRequest,
    ) -> Result<CrmCompanyResponse> {
        self.post(&self.config.storage_base_url, "/crm/companies", request)
            .await
    }

    pub async fn get_crm_company(&self, company_id: &str) -> Result<CrmCompanyResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!("/crm/companies/{}", urlencoding::encode(company_id)),
        )
        .await
    }

    pub async fn create_crm_contact(
        &self,
        company_id: &str,
        request: &CreateCrmContactRequest,
    ) -> Result<CrmContactResponse> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/crm/companies/{}/contacts",
                urlencoding::encode(company_id)
            ),
            request,
        )
        .await
    }

    pub async fn get_crm_contact(&self, contact_id: &str) -> Result<CrmContactResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!("/crm/contacts/{}", urlencoding::encode(contact_id)),
        )
        .await
    }

    pub async fn set_crm_company_name(&self, company_id: &str, name: &str) -> Result<()> {
        self.put_empty(
            &self.config.storage_base_url,
            &format!("/crm/companies/{}/name", urlencoding::encode(company_id)),
            &SetCrmNameRequest { name: name.into() },
        )
        .await
    }

    pub async fn set_crm_contact_name(&self, contact_id: &str, name: &str) -> Result<()> {
        self.put_empty(
            &self.config.storage_base_url,
            &format!("/crm/contacts/{}/name", urlencoding::encode(contact_id)),
            &SetCrmNameRequest { name: name.into() },
        )
        .await
    }

    pub async fn set_crm_company_hidden(&self, company_id: &str, hidden: bool) -> Result<()> {
        self.put_empty(
            &self.config.storage_base_url,
            &format!("/crm/companies/{}/hidden", urlencoding::encode(company_id)),
            &SetCrmHiddenRequest { hidden },
        )
        .await
    }

    pub async fn set_crm_contact_hidden(&self, contact_id: &str, hidden: bool) -> Result<()> {
        self.put_empty(
            &self.config.storage_base_url,
            &format!("/crm/contacts/{}/hidden", urlencoding::encode(contact_id)),
            &SetCrmHiddenRequest { hidden },
        )
        .await
    }

    pub async fn set_crm_company_email_sync(
        &self,
        company_id: &str,
        email_sync: bool,
    ) -> Result<()> {
        self.put_empty(
            &self.config.storage_base_url,
            &format!(
                "/crm/companies/{}/email-sync",
                urlencoding::encode(company_id)
            ),
            &SetCrmEmailSyncRequest { email_sync },
        )
        .await
    }

    pub async fn list_crm_comments(
        &self,
        entity_type: CrmCommentEntityType,
        entity_id: &str,
    ) -> Result<Vec<CrmCommentThread>> {
        self.get(
            &self.config.storage_base_url,
            &format!(
                "/crm/comments/{}/{}",
                entity_type.as_path_segment(),
                urlencoding::encode(entity_id)
            ),
        )
        .await
    }

    pub async fn create_crm_comment(
        &self,
        entity_type: CrmCommentEntityType,
        entity_id: &str,
        request: &CreateCrmCommentRequest,
    ) -> Result<CrmCommentThread> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/crm/comments/{}/{}",
                entity_type.as_path_segment(),
                urlencoding::encode(entity_id)
            ),
            request,
        )
        .await
    }

    pub async fn edit_crm_comment(
        &self,
        comment_id: &str,
        request: &EditCrmCommentRequest,
    ) -> Result<CrmComment> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/crm/comment/{}", urlencoding::encode(comment_id)),
            request,
        )
        .await
    }

    pub async fn delete_crm_comment(&self, comment_id: &str) -> Result<DeleteCrmCommentResult> {
        self.delete(
            &self.config.storage_base_url,
            &format!("/crm/comment/{}", urlencoding::encode(comment_id)),
        )
        .await
    }

    pub async fn get_crm_team_settings(&self) -> Result<CrmTeamSettingsResponse> {
        self.get(&self.config.storage_base_url, "/crm/settings")
            .await
    }

    pub async fn update_crm_team_settings(
        &self,
        request: &UpdateCrmTeamSettingsRequest,
    ) -> Result<CrmTeamSettingsResponse> {
        self.put(&self.config.storage_base_url, "/crm/settings", request)
            .await
    }

    pub async fn list_saved_views(&self) -> Result<SavedViewsResponse> {
        self.get(&self.config.storage_base_url, "/saved_views")
            .await
    }

    pub async fn create_saved_view(&self, request: &CreateSavedViewRequest) -> Result<SavedView> {
        self.post(&self.config.storage_base_url, "/saved_views", request)
            .await
    }

    pub async fn patch_saved_view(
        &self,
        saved_view_id: &str,
        request: &PatchSavedViewRequest,
    ) -> Result<()> {
        let url = format!(
            "{}/saved_views/{}",
            self.config.storage_base_url,
            urlencoding::encode(saved_view_id)
        );
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn delete_saved_view(&self, saved_view_id: &str) -> Result<()> {
        let url = format!(
            "{}/saved_views/{}",
            self.config.storage_base_url,
            urlencoding::encode(saved_view_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn exclude_default_view(&self, default_view_id: &str) -> Result<()> {
        let request = ExcludeDefaultViewRequest {
            default_view_id: default_view_id.to_owned(),
        };
        let body = serde_json::to_string(&request)?;
        let url = format!(
            "{}/saved_views/exclude_default",
            self.config.storage_base_url
        );
        let mut http_request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    // === Email Service ===

    pub async fn list_email_thread_previews(
        &self,
        view: &str,
    ) -> Result<EmailThreadPreviewsResponse> {
        self.get(
            &self.config.email_base_url,
            &format!(
                "/email/threads/previews/cursor/{}",
                urlencoding::encode(view)
            ),
        )
        .await
    }

    pub async fn get_email_thread(&self, thread_id: &str) -> Result<GetEmailThreadResponse> {
        self.get(
            &self.config.email_base_url,
            &format!(
                "/email/threads/{}?limit=100",
                urlencoding::encode(thread_id)
            ),
        )
        .await
    }

    pub async fn get_email_messages(&self, thread_id: &str) -> Result<Vec<ParsedEmailMessage>> {
        self.get(
            &self.config.email_base_url,
            &format!("/email/threads/{}/messages", urlencoding::encode(thread_id)),
        )
        .await
    }

    pub async fn get_email_attachment(
        &self,
        attachment_id: &str,
    ) -> Result<GetEmailAttachmentResponse> {
        self.get(
            &self.config.email_base_url,
            &format!("/email/attachments/{}", urlencoding::encode(attachment_id)),
        )
        .await
    }

    pub async fn get_email_attachment_document_id(
        &self,
        attachment_id: &str,
    ) -> Result<GetEmailAttachmentDocumentIdResponse> {
        self.get(
            &self.config.email_base_url,
            &format!(
                "/email/attachments/{}/document_id",
                urlencoding::encode(attachment_id)
            ),
        )
        .await
    }

    pub async fn send_email(
        &self,
        to: EmailContactInput,
        subject: &str,
        body_text: &str,
    ) -> Result<SendEmailMessageResponse> {
        self.send_email_message(EmailDraftInput {
            subject: subject.to_owned(),
            to: vec![to],
            cc: Vec::new(),
            bcc: Vec::new(),
            thread_db_id: None,
            replying_to_id: None,
            body_text: Some(body_text.to_owned()),
            body_html: None,
            ..Default::default()
        })
        .await
    }

    pub async fn reply_email(
        &self,
        thread_id: &str,
        replying_to_id: &str,
        to: EmailContactInput,
        subject: &str,
        body_text: &str,
    ) -> Result<SendEmailMessageResponse> {
        self.send_email_message(EmailDraftInput {
            subject: subject.to_owned(),
            to: vec![to],
            cc: Vec::new(),
            bcc: Vec::new(),
            thread_db_id: Some(thread_id.to_owned()),
            replying_to_id: Some(replying_to_id.to_owned()),
            body_text: Some(body_text.to_owned()),
            body_html: None,
            ..Default::default()
        })
        .await
    }

    pub async fn send_email_message(
        &self,
        message: EmailDraftInput,
    ) -> Result<SendEmailMessageResponse> {
        self.send_email_message_for_link(message, None).await
    }

    pub async fn send_email_message_for_link(
        &self,
        message: EmailDraftInput,
        link_id: Option<&str>,
    ) -> Result<SendEmailMessageResponse> {
        let url = format!(
            "{}/email/messages",
            self.config.email_base_url.trim_end_matches('/')
        );
        let body = serde_json::to_string(&SendMessageRequest { message })?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    /// Creates or updates a server-backed draft. Supplying `draft.db_id`
    /// updates that draft, matching Macro's autosave contract.
    pub async fn create_email_draft(
        &self,
        draft: EmailDraftInput,
        send_time: Option<String>,
    ) -> Result<CreateEmailDraftResponse> {
        self.create_email_draft_for_link(draft, send_time, None)
            .await
    }

    pub async fn create_email_draft_for_link(
        &self,
        draft: EmailDraftInput,
        send_time: Option<String>,
        link_id: Option<&str>,
    ) -> Result<CreateEmailDraftResponse> {
        let url = format!(
            "{}/email/drafts",
            self.config.email_base_url.trim_end_matches('/')
        );
        let body = serde_json::to_string(&CreateEmailDraftRequest { draft, send_time })?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn delete_email_draft(&self, draft_id: &str) -> Result<()> {
        self.delete_email_draft_for_link(draft_id, None).await
    }

    pub async fn delete_email_draft_for_link(
        &self,
        draft_id: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/email/drafts/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn schedule_email_draft(
        &self,
        draft_id: &str,
        send_time: &str,
    ) -> Result<ScheduleEmailDraftResponse> {
        self.schedule_email_draft_for_link(draft_id, send_time, None)
            .await
    }

    pub async fn schedule_email_draft_for_link(
        &self,
        draft_id: &str,
        send_time: &str,
        link_id: Option<&str>,
    ) -> Result<ScheduleEmailDraftResponse> {
        let url = format!(
            "{}/email/drafts/scheduled/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id)
        );
        let body = serde_json::to_string(&ScheduleEmailDraftRequest {
            send_time: send_time.to_owned(),
        })?;
        let mut request = self
            .http
            .put(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn unschedule_email_draft(&self, draft_id: &str) -> Result<()> {
        self.unschedule_email_draft_for_link(draft_id, None).await
    }

    pub async fn unschedule_email_draft_for_link(
        &self,
        draft_id: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/email/drafts/scheduled/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_scheduled_email_drafts(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<ScheduledEmailDraftsResponse> {
        self.list_scheduled_email_drafts_for_link(limit, offset, None)
            .await
    }

    pub async fn list_scheduled_email_drafts_for_link(
        &self,
        limit: u32,
        offset: u32,
        link_id: Option<&str>,
    ) -> Result<ScheduledEmailDraftsResponse> {
        let url = format!(
            "{}/email/drafts/scheduled?limit={limit}&offset={offset}",
            self.config.email_base_url.trim_end_matches('/')
        );
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn prepare_email_draft_attachment(
        &self,
        draft_id: &str,
        request: &AddEmailDraftAttachmentRequest,
    ) -> Result<AddEmailDraftAttachmentResponse> {
        self.post(
            &self.config.email_base_url,
            &format!(
                "/email/drafts/{}/attachments",
                urlencoding::encode(draft_id)
            ),
            request,
        )
        .await
    }

    pub async fn remove_email_draft_attachment(
        &self,
        draft_id: &str,
        attachment_id: &str,
    ) -> Result<()> {
        self.remove_email_draft_attachment_for_link(draft_id, attachment_id, None)
            .await
    }

    pub async fn remove_email_draft_attachment_for_link(
        &self,
        draft_id: &str,
        attachment_id: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/email/drafts/{}/attachments/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id),
            urlencoding::encode(attachment_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn add_forwarded_email_attachment(
        &self,
        draft_id: &str,
        attachment_id: &str,
        link_id: Option<&str>,
    ) -> Result<AddForwardedEmailAttachmentResponse> {
        let url = format!(
            "{}/email/drafts/{}/forwarded-attachments",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id)
        );
        let body = serde_json::to_string(&AddForwardedEmailAttachmentRequest {
            attachment_id: attachment_id.to_owned(),
        })?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn remove_forwarded_email_attachment(
        &self,
        draft_id: &str,
        attachment_id: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        let url = format!(
            "{}/email/drafts/{}/forwarded-attachments/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id),
            urlencoding::encode(attachment_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    /// Registers and uploads one attachment to an existing server-backed draft.
    /// Macro expects a lowercase SHA-256 digest in the registration request and
    /// the same digest encoded as base64 in the S3 checksum header.
    pub async fn upload_email_draft_attachment(
        &self,
        draft_id: &str,
        link_id: Option<&str>,
        file_name: &str,
        bytes: Vec<u8>,
    ) -> Result<AddEmailDraftAttachmentResponse> {
        let digest = Sha256::digest(&bytes);
        let sha = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        let url = format!(
            "{}/email/drafts/{}/attachments",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(draft_id)
        );
        let body = serde_json::to_string(&AddEmailDraftAttachmentRequest {
            file_name: file_name.to_owned(),
            sha,
            size: bytes.len() as u64,
        })?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        let attachment: AddEmailDraftAttachmentResponse = self.execute(request).await?;
        let upload_url = attachment.upload_url.clone();
        let content_type = attachment.content_type.clone();
        let checksum = base64::engine::general_purpose::STANDARD.encode(digest);
        let (status, text) = reqwest_client::runtime()
            .spawn(async move {
                let response = Client::new()
                    .put(upload_url)
                    .header("content-type", content_type)
                    .header("x-amz-checksum-sha256", checksum)
                    .body(bytes)
                    .send()
                    .await?;
                let status = response.status();
                let text = response.text().await?;
                Ok::<_, reqwest::Error>((status, text))
            })
            .await
            .context("attachment upload runtime failed")?
            .context("attachment upload request failed")?;
        if !status.is_success() {
            let _ = self
                .remove_email_draft_attachment_for_link(
                    draft_id,
                    &attachment.attachment_id,
                    link_id,
                )
                .await;
            anyhow::bail!("attachment upload HTTP {status}: {text}");
        }
        Ok(attachment)
    }

    pub async fn list_email_links(&self) -> Result<ListEmailLinksResponse> {
        self.get(&self.config.email_base_url, "/email/links").await
    }

    /// Starts Macro's authenticated multi-inbox Gmail consent flow. The returned
    /// URL must be opened in the system browser; `original_url` receives the
    /// generated `link_id` after consent.
    pub async fn init_gmail_link(
        &self,
        original_url: &str,
        scopes: &str,
    ) -> Result<InitGmailLinkResponse> {
        let path = format!(
            "/link/gmail?original_url={}&scopes={}",
            urlencoding::encode(original_url),
            urlencoding::encode(scopes),
        );
        let url = format!("{}{}", self.config.auth_base_url, path);
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute(request).await
    }

    /// Finalizes an inbox after `/link/gmail` consent. `force_share` is only
    /// used after the user confirms a 409 shared-inbox conflict.
    pub async fn init_email_link(
        &self,
        link_id: Option<&str>,
        force_share: bool,
    ) -> Result<InitEmailLinkResponse> {
        let mut query = Vec::new();
        if let Some(link_id) = link_id {
            query.push(format!("link_id={}", urlencoding::encode(link_id)));
        }
        if force_share {
            query.push("force_share=true".to_owned());
        }
        let path = if query.is_empty() {
            "/email/init".to_owned()
        } else {
            format!("/email/init?{}", query.join("&"))
        };
        let url = format!("{}{}", self.config.email_base_url, path);
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute(request).await
    }

    pub async fn health_check_email_links(&self) -> Result<()> {
        let url = format!(
            "{}/email/links/health-check",
            self.config.email_base_url.trim_end_matches('/')
        );
        let mut request = self.http.post(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn delete_email_link(&self, link_id: &str) -> Result<()> {
        let url = format!(
            "{}/email/links/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(link_id),
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn disable_email_link_calendar(&self, link_id: &str) -> Result<()> {
        let url = format!(
            "{}/email/links/{}/calendar",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(link_id),
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn patch_email_settings(
        &self,
        settings: EmailSettings,
    ) -> Result<PatchEmailSettingsResponse> {
        self.patch(
            &self.config.email_base_url,
            "/email/settings",
            &PatchEmailSettingsRequest { settings },
        )
        .await
    }

    pub async fn disable_email_sync(&self) -> Result<()> {
        let url = format!(
            "{}/email/sync",
            self.config.email_base_url.trim_end_matches('/')
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn resync_email_link(&self, link_id: &str) -> Result<ResyncEmailLinkResponse> {
        self.post(
            &self.config.email_base_url,
            &format!("/email/links/{}/resync", urlencoding::encode(link_id)),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn set_thread_archived(
        &self,
        thread_id: &str,
        archived: bool,
    ) -> Result<serde_json::Value> {
        self.set_thread_archived_for_link(thread_id, archived, None)
            .await
    }

    pub async fn set_thread_archived_for_link(
        &self,
        thread_id: &str,
        archived: bool,
        link_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/email/threads/{}/archived",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(thread_id)
        );
        let body = serde_json::to_string(&ArchiveThreadRequest { value: archived })?;
        let mut request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn set_thread_label(
        &self,
        thread_id: &str,
        label_id: &str,
        value: bool,
    ) -> Result<serde_json::Value> {
        self.set_thread_label_for_link(thread_id, label_id, value, None)
            .await
    }

    pub async fn set_thread_label_for_link(
        &self,
        thread_id: &str,
        label_id: &str,
        value: bool,
        link_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/email/threads/{}/labels",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(thread_id)
        );
        let body = serde_json::to_string(&serde_json::json!({
            "label_id": label_id,
            "value": value,
        }))?;
        let mut request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn mark_thread_seen(&self, thread_id: &str) -> Result<serde_json::Value> {
        self.mark_thread_seen_for_link(thread_id, None).await
    }

    pub async fn mark_thread_seen_for_link(
        &self,
        thread_id: &str,
        link_id: Option<&str>,
    ) -> Result<serde_json::Value> {
        let url = format!(
            "{}/email/threads/{}/seen",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(thread_id)
        );
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body("{}");
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn list_labels(&self) -> Result<ListEmailLabelsResponse> {
        self.get(&self.config.email_base_url, "/email/labels").await
    }

    pub async fn list_email_filters(
        &self,
        link_id: Option<&str>,
    ) -> Result<ListEmailFiltersResponse> {
        let url = format!(
            "{}/email/filters",
            self.config.email_base_url.trim_end_matches('/')
        );
        let mut request = self.http.get(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn upsert_email_filter(
        &self,
        request_body: &UpsertEmailFilterRequest,
        link_id: Option<&str>,
    ) -> Result<UpsertEmailFilterResponse> {
        let url = format!(
            "{}/email/filters",
            self.config.email_base_url.trim_end_matches('/')
        );
        let body = serde_json::to_string(request_body)?;
        let mut request = self
            .http
            .put(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute(request).await
    }

    pub async fn delete_email_filter(&self, filter_id: &str, link_id: Option<&str>) -> Result<()> {
        let url = format!(
            "{}/email/filters/{}",
            self.config.email_base_url.trim_end_matches('/'),
            urlencoding::encode(filter_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    async fn set_email_sender_blocked(
        &self,
        email_address: &str,
        blocked: bool,
        link_id: Option<&str>,
    ) -> Result<()> {
        let action = if blocked { "block" } else { "unblock" };
        let url = format!(
            "{}/email/contacts/{action}",
            self.config.email_base_url.trim_end_matches('/')
        );
        let body = serde_json::to_string(&serde_json::json!({
            "email_address": email_address,
        }))?;
        let mut request = self
            .http
            .post(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(token);
        }
        if let Some(link_id) = link_id {
            request = request.header("x-email-link-id", link_id);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn block_email_sender(
        &self,
        email_address: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        self.set_email_sender_blocked(email_address, true, link_id)
            .await
    }

    pub async fn unblock_email_sender(
        &self,
        email_address: &str,
        link_id: Option<&str>,
    ) -> Result<()> {
        self.set_email_sender_blocked(email_address, false, link_id)
            .await
    }

    // === Calendar ===

    pub async fn list_calendars(&self) -> Result<ListCalendarsResponse> {
        self.get(&self.config.email_base_url, "/calendar/calendars")
            .await
    }

    pub async fn create_calendar_event(
        &self,
        request: &CreateCalendarEventRequest,
    ) -> Result<CalendarEvent> {
        self.post(&self.config.email_base_url, "/calendar/events", request)
            .await
    }

    pub async fn update_calendar_event(
        &self,
        event_id: &str,
        request: &UpdateCalendarEventRequest,
    ) -> Result<CalendarEvent> {
        self.patch(
            &self.config.email_base_url,
            &format!("/calendar/events/{}", urlencoding::encode(event_id)),
            request,
        )
        .await
    }

    pub async fn rsvp_calendar_event(
        &self,
        event_id: &str,
        request: &RsvpCalendarEventRequest,
    ) -> Result<CalendarEvent> {
        self.put(
            &self.config.email_base_url,
            &format!("/calendar/events/{}/rsvp", urlencoding::encode(event_id)),
            request,
        )
        .await
    }

    pub async fn delete_calendar_event(
        &self,
        event_id: &str,
        scope: Option<&str>,
        recurrence_id: Option<&str>,
    ) -> Result<()> {
        let mut path = format!("/calendar/events/{}", urlencoding::encode(event_id));
        let mut query = Vec::new();
        if let Some(scope) = scope {
            query.push(format!("scope={}", urlencoding::encode(scope)));
        }
        if let Some(recurrence_id) = recurrence_id {
            query.push(format!(
                "recurrenceId={}",
                urlencoding::encode(recurrence_id)
            ));
        }
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query.join("&"));
        }
        let url = format!("{}{}", self.config.email_base_url, path);
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    // === Search ===

    /// Runs unified search using Macro's generated request/response contract.
    pub async fn search(
        &self,
        request: &UnifiedSearchRequest,
        page_size: u32,
        cursor: Option<&str>,
    ) -> Result<UnifiedSearchResponse> {
        let mut path = format!("/search?page_size={page_size}");
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.post(&self.config.search_base_url, &path, request)
            .await
    }

    // === Agent ===

    /// Creates a managed or external session using Macro's generated request shape.
    pub async fn create_agent_session(
        &self,
        request: &CreateAgentSessionRequest,
    ) -> Result<CreateAgentSessionResponse> {
        self.post(&self.config.agent_base_url, "/agent-sessions", request)
            .await
    }

    pub async fn get_agent_session(&self, session_id: &str) -> Result<AgentSessionResponse> {
        let path = format!("/agent-sessions/{}", urlencoding::encode(session_id));
        self.get(&self.config.agent_base_url, &path).await
    }

    pub async fn get_agent_session_log(&self, session_id: &str) -> Result<AgentSessionLogResponse> {
        self.get(
            &self.config.agent_base_url,
            &format!("/agent-sessions/{}/log", urlencoding::encode(session_id)),
        )
        .await
    }

    pub async fn control_agent_session(
        &self,
        session_id: &str,
        action: &AgentAction,
    ) -> Result<String> {
        self.post(
            &self.config.agent_base_url,
            &format!(
                "/agent-sessions/{}/control",
                urlencoding::encode(session_id)
            ),
            action,
        )
        .await
    }

    pub async fn rename_agent_session(&self, session_id: &str, name: &str) -> Result<()> {
        let url = format!(
            "{}/agent-sessions/{}/name",
            self.config.agent_base_url,
            urlencoding::encode(session_id)
        );
        let body = serde_json::to_string(&serde_json::json!({ "name": name }))?;
        let mut request = self
            .http
            .put(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn get_default_agent_sandbox_size(&self) -> Result<SandboxSizeBody> {
        self.get(&self.config.agent_base_url, "/agent-sandbox-size")
            .await
    }

    pub async fn set_default_agent_sandbox_size(
        &self,
        size: SandboxSize,
    ) -> Result<SandboxSizeBody> {
        self.put(
            &self.config.agent_base_url,
            "/agent-sandbox-size",
            &SandboxSizeBody { size },
        )
        .await
    }

    pub async fn set_agent_session_sandbox_size(
        &self,
        session_id: &str,
        size: SandboxSize,
    ) -> Result<SandboxSizeBody> {
        self.put(
            &self.config.agent_base_url,
            &format!(
                "/agent-sessions/{}/sandbox-size",
                urlencoding::encode(session_id)
            ),
            &SandboxSizeBody { size },
        )
        .await
    }

    // === Scheduled Actions ===

    pub async fn list_scheduled_actions(&self) -> Result<Vec<ScheduledAction>> {
        self.get(&self.config.scheduled_action_base_url, "/scheduled-actions")
            .await
    }

    pub async fn create_scheduled_action(
        &self,
        request: &CreateScheduledAction,
    ) -> Result<ScheduledAction> {
        self.post(
            &self.config.scheduled_action_base_url,
            "/scheduled-actions",
            request,
        )
        .await
    }

    pub async fn update_scheduled_action(
        &self,
        action_id: &str,
        request: &UpdateScheduledAction,
    ) -> Result<ScheduledAction> {
        self.put(
            &self.config.scheduled_action_base_url,
            &format!("/scheduled-actions/{}", urlencoding::encode(action_id)),
            request,
        )
        .await
    }

    pub async fn delete_scheduled_action(&self, action_id: &str) -> Result<()> {
        let url = format!(
            "{}/scheduled-actions/{}",
            self.config.scheduled_action_base_url,
            urlencoding::encode(action_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn execute_scheduled_action(&self, action_id: &str) -> Result<InProgressExecution> {
        self.post(
            &self.config.scheduled_action_base_url,
            &format!(
                "/scheduled-actions/{}/execute",
                urlencoding::encode(action_id)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn list_scheduled_action_history(
        &self,
        action_id: &str,
    ) -> Result<Vec<ActionExecutionRecord>> {
        self.get(
            &self.config.scheduled_action_base_url,
            &format!(
                "/scheduled-actions/{}/history",
                urlencoding::encode(action_id)
            ),
        )
        .await
    }

    pub async fn delete_agent_session(&self, session_id: &str) -> Result<()> {
        let url = format!(
            "{}/agent-sessions/{}",
            self.config.agent_base_url,
            urlencoding::encode(session_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    // === Calls ===

    pub async fn list_active_calls(&self) -> Result<Vec<ActiveCallSummary>> {
        Ok(self
            .get::<ActiveCallsResponse>(&self.config.storage_base_url, "/call/active")
            .await?
            .calls)
    }

    /// Creates or joins the server-side call session. The caller must connect the
    /// returned token to the native RTC layer before presenting an in-call state.
    pub async fn get_or_create_call(&self, channel_id: &str) -> Result<CallTokenResponse> {
        self.get(
            &self.config.storage_base_url,
            &format!("/call/{}", urlencoding::encode(channel_id)),
        )
        .await
    }

    /// Returns `None` for the generated SDK's 204 "no active call" response.
    pub async fn check_active_call(&self, channel_id: &str) -> Result<Option<CallActiveResponse>> {
        self.get_optional(
            &self.config.storage_base_url,
            &format!("/call/{}/active", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn get_ring_status(&self, call_id: &str) -> Result<RingStatus> {
        Ok(self
            .get::<RingStatusResponse>(
                &self.config.storage_base_url,
                &format!("/call/ring-status/{}", urlencoding::encode(call_id)),
            )
            .await?
            .status)
    }

    pub async fn leave_call(&self, channel_id: &str) -> Result<LeaveCallResponse> {
        self.delete(
            &self.config.storage_base_url,
            &format!("/call/{}", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn get_call_record(&self, call_id: &str) -> Result<CallRecord> {
        self.get(
            &self.config.storage_base_url,
            &format!("/call/record/{}", urlencoding::encode(call_id)),
        )
        .await
    }

    pub async fn edit_call_record(
        &self,
        call_id: &str,
        request: &EditCallRecordRequest,
    ) -> Result<()> {
        let url = format!(
            "{}/call/record/{}",
            self.config.storage_base_url,
            urlencoding::encode(call_id)
        );
        let body = serde_json::to_string(request)?;
        let mut request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn edit_call_transcript(
        &self,
        call_id: &str,
        request: &EditCallTranscriptRequest,
    ) -> Result<()> {
        let url = format!(
            "{}/call/record/{}/transcript",
            self.config.storage_base_url,
            urlencoding::encode(call_id)
        );
        let body = serde_json::to_string(request)?;
        let mut http_request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            http_request = http_request.bearer_auth(&token);
        }
        self.execute_empty_ok(http_request).await
    }

    pub async fn toggle_call_share_with_team(&self, call_id: &str) -> Result<bool> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/call/record/{}/share-with-team/toggle",
                urlencoding::encode(call_id)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn delete_call_record(&self, call_id: &str) -> Result<()> {
        let url = format!(
            "{}/call/record/{}",
            self.config.storage_base_url,
            urlencoding::encode(call_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    // === Integrations ===

    pub async fn get_self_bot(&self) -> Result<Option<Bot>> {
        self.get_optional(&self.config.storage_base_url, "/bots/me")
            .await
    }

    pub async fn list_bots(&self) -> Result<Vec<Bot>> {
        self.get(&self.config.storage_base_url, "/bots").await
    }

    pub async fn create_bot(&self, request: &CreateBotRequest) -> Result<Bot> {
        self.post(&self.config.storage_base_url, "/bots", request)
            .await
    }

    pub async fn get_bot(&self, bot_id: &str) -> Result<Bot> {
        self.get(
            &self.config.storage_base_url,
            &format!("/bots/{}", urlencoding::encode(bot_id)),
        )
        .await
    }

    pub async fn patch_bot(&self, bot_id: &str, request: &PatchBotRequest) -> Result<Bot> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/bots/{}", urlencoding::encode(bot_id)),
            request,
        )
        .await
    }

    pub async fn delete_bot(&self, bot_id: &str) -> Result<()> {
        let url = format!(
            "{}/bots/{}",
            self.config.storage_base_url,
            urlencoding::encode(bot_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn create_bot_token(
        &self,
        bot_id: &str,
        request: &CreateBotTokenRequest,
    ) -> Result<CreateBotTokenResponse> {
        self.post(
            &self.config.storage_base_url,
            &format!("/bots/{}/tokens", urlencoding::encode(bot_id)),
            request,
        )
        .await
    }

    pub async fn list_bot_tokens(&self, bot_id: &str) -> Result<Vec<BotToken>> {
        self.get(
            &self.config.storage_base_url,
            &format!("/bots/{}/tokens", urlencoding::encode(bot_id)),
        )
        .await
    }

    pub async fn revoke_bot_token(&self, bot_id: &str, token_id: &str) -> Result<()> {
        let url = format!(
            "{}/bots/{}/tokens/{}",
            self.config.storage_base_url,
            urlencoding::encode(bot_id),
            urlencoding::encode(token_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_channel_bots(&self, channel_id: &str) -> Result<Vec<Bot>> {
        self.get(
            &self.config.storage_base_url,
            &format!("/channels/{}/bots", urlencoding::encode(channel_id)),
        )
        .await
    }

    pub async fn add_bot_to_channel(&self, channel_id: &str, bot_id: &str) -> Result<()> {
        self.post_empty(
            &self.config.storage_base_url,
            &format!("/channels/{}/bots", urlencoding::encode(channel_id)),
            &serde_json::json!({ "bot_id": bot_id }),
        )
        .await
    }

    pub async fn list_bot_channels(&self, bot_id: &str) -> Result<Vec<BotChannel>> {
        self.get(
            &self.config.storage_base_url,
            &format!("/bots/{}/channels", urlencoding::encode(bot_id)),
        )
        .await
    }

    pub async fn create_channel_scoped_bot(
        &self,
        channel_id: &str,
        request: &CreateChannelScopedBotRequest,
    ) -> Result<CreateChannelScopedBotResponse> {
        self.post(
            &self.config.storage_base_url,
            &format!("/channels/{}/bots/scoped", urlencoding::encode(channel_id)),
            request,
        )
        .await
    }

    pub async fn remove_bot_from_channel(&self, bot_id: &str, channel_id: &str) -> Result<()> {
        let url = format!(
            "{}/bots/{}/channels/{}",
            self.config.storage_base_url,
            urlencoding::encode(bot_id),
            urlencoding::encode(channel_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn list_webhooks(&self) -> Result<ListWebhooksResponse> {
        self.get(&self.config.storage_base_url, "/webhook/webhooks")
            .await
    }

    pub async fn get_webhook(&self, webhook_id: &str) -> Result<Webhook> {
        self.get(
            &self.config.storage_base_url,
            &format!("/webhook/webhooks/{}", urlencoding::encode(webhook_id)),
        )
        .await
    }

    pub async fn create_webhook(
        &self,
        request: &CreateWebhookRequest,
    ) -> Result<CreateWebhookResponse> {
        self.post(&self.config.storage_base_url, "/webhook/webhooks", request)
            .await
    }

    pub async fn patch_webhook(
        &self,
        webhook_id: &str,
        request: &PatchWebhookRequest,
    ) -> Result<Webhook> {
        self.patch(
            &self.config.storage_base_url,
            &format!("/webhook/webhooks/{}", urlencoding::encode(webhook_id)),
            request,
        )
        .await
    }

    pub async fn delete_webhook(&self, webhook_id: &str) -> Result<()> {
        let url = format!(
            "{}/webhook/webhooks/{}",
            self.config.storage_base_url,
            urlencoding::encode(webhook_id)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn validate_webhook(&self, webhook_id: &str) -> Result<ValidateWebhookResponse> {
        self.post(
            &self.config.storage_base_url,
            &format!(
                "/webhook/webhooks/{}/validate",
                urlencoding::encode(webhook_id)
            ),
            &serde_json::json!({}),
        )
        .await
    }

    pub async fn get_memory(&self) -> Result<MemoryResponse> {
        self.get(&self.config.cognition_base_url, "/memory").await
    }

    pub async fn get_onboarding_state(&self) -> Result<OnboardingState> {
        self.get(&self.config.cognition_base_url, "/onboarding")
            .await
    }

    pub async fn complete_onboarding(&self, skipped: bool) -> Result<OnboardingRow> {
        self.post(
            &self.config.cognition_base_url,
            "/onboarding/complete",
            &CompleteOnboardingRequest { skipped },
        )
        .await
    }

    pub async fn patch_user_tutorial(&self, tutorial_complete: bool) -> Result<()> {
        let url = format!("{}/user/tutorial", self.config.auth_base_url);
        let body = serde_json::to_string(&PatchUserTutorialRequest { tutorial_complete })?;
        let mut request = self
            .http
            .patch(url)
            .header("content-type", "application/json")
            .body(body);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    /// Sends the same optional, privacy-scoped deletion feedback event as the
    /// web client before account deletion. The PostHog project key is public
    /// client configuration and may be supplied at build or launch time.
    pub async fn capture_account_deletion_feedback(
        &self,
        distinct_id: &str,
        reason: &str,
        feedback: Option<&str>,
    ) -> Result<bool> {
        let api_key = std::env::var("MACRO_POSTHOG_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| option_env!("MACRO_POSTHOG_API_KEY").map(ToOwned::to_owned));
        let Some(api_key) = api_key else {
            return Ok(false);
        };
        let body = serde_json::to_string(&account_deletion_feedback_payload(
            &api_key,
            distinct_id,
            reason,
            feedback,
        ))?;
        let request = self
            .http
            .post("https://macro-prox.macroverse.workers.dev/i/ph/capture/")
            .header("content-type", "application/json")
            .body(body);
        self.execute_empty_ok(request).await?;
        Ok(true)
    }

    pub async fn get_import_state(&self) -> Result<ImportState> {
        self.get(&self.config.cognition_base_url, "/import/state")
            .await
    }

    pub async fn run_import(&self, request: &RunImportRequest) -> Result<RunImportOutcome> {
        self.post(&self.config.cognition_base_url, "/import/run", request)
            .await
    }

    pub async fn retry_import_gather(&self, source: ImportSource) -> Result<()> {
        self.post_no_body_empty(
            &self.config.cognition_base_url,
            &format!(
                "/import/runs/{}/retry",
                urlencoding::encode(source.as_str())
            ),
        )
        .await
    }

    pub async fn dismiss_import_run(&self, source: ImportSource) -> Result<()> {
        self.post_no_body_empty(
            &self.config.cognition_base_url,
            &format!(
                "/import/runs/{}/dismiss",
                urlencoding::encode(source.as_str())
            ),
        )
        .await
    }

    pub async fn list_mcp_servers(&self) -> Result<Vec<McpServerResponse>> {
        self.get(&self.config.cognition_base_url, "/mcp/servers")
            .await
    }

    pub async fn add_mcp_server(&self, request: &AddMcpServerRequest) -> Result<McpServerResponse> {
        self.post(&self.config.cognition_base_url, "/mcp/servers", request)
            .await
    }

    pub async fn update_mcp_server(
        &self,
        request: &UpdateMcpServerRequest,
    ) -> Result<McpServerResponse> {
        self.put(&self.config.cognition_base_url, "/mcp/servers", request)
            .await
    }

    pub async fn delete_mcp_server(&self, server_url: &str) -> Result<()> {
        let url = format!(
            "{}/mcp/servers?url={}",
            self.config.cognition_base_url,
            urlencoding::encode(server_url)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn start_mcp_auth(
        &self,
        request: &StartMcpAuthRequest,
    ) -> Result<StartMcpAuthResponse> {
        self.post(
            &self.config.cognition_base_url,
            "/mcp/servers/auth/start",
            request,
        )
        .await
    }

    pub async fn list_pipedream_connections(&self) -> Result<Vec<PipedreamConnectionResponse>> {
        self.get(
            &self.config.cognition_base_url,
            "/pipedream/mcp/connections",
        )
        .await
    }

    pub async fn set_pipedream_connection_enabled(
        &self,
        app_slug: &str,
        enabled: bool,
    ) -> Result<PipedreamConnectionResponse> {
        self.put(
            &self.config.cognition_base_url,
            "/pipedream/mcp/connections",
            &serde_json::json!({ "app_slug": app_slug, "enabled": enabled }),
        )
        .await
    }

    pub async fn delete_pipedream_connection(&self, app_slug: &str) -> Result<()> {
        let url = format!(
            "{}/pipedream/mcp/connections?app_slug={}",
            self.config.cognition_base_url,
            urlencoding::encode(app_slug)
        );
        let mut request = self.http.delete(url);
        if let Some(token) = self.token() {
            request = request.bearer_auth(&token);
        }
        self.execute_empty_ok(request).await
    }

    pub async fn browse_pipedream_catalog(
        &self,
        search: Option<&str>,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<PipedreamCatalogResponse> {
        let mut path = format!("/pipedream/mcp/catalog?limit={limit}");
        if let Some(search) = search {
            path.push_str("&search=");
            path.push_str(&urlencoding::encode(search));
        }
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor));
        }
        self.get(&self.config.cognition_base_url, &path).await
    }

    pub async fn create_pipedream_token(&self) -> Result<PipedreamTokenResponse> {
        self.post(
            &self.config.cognition_base_url,
            "/pipedream/mcp/token",
            &serde_json::json!({}),
        )
        .await
    }

    // === WebSocket Connection ===

    pub fn websocket_url(&self) -> String {
        let base = self
            .config
            .connection_base_url
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let token = self.token().unwrap_or_default();
        format!(
            "{}/?macro-api-token={}",
            base.trim_end_matches('/'),
            urlencoding::encode(&token)
        )
    }

    /// Authenticated GraphQL-over-WebSocket endpoint used by Macro's live Soup
    /// entity subscription. This is intentionally separate from the generic
    /// connection gateway: property and sharing updates are published by DSS.
    pub fn soup_websocket_url(&self) -> String {
        let base = self
            .config
            .storage_base_url
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        let token = self.token().unwrap_or_default();
        format!(
            "{}/items/soup/graphql/ws?macro-api-token={}",
            base.trim_end_matches('/'),
            urlencoding::encode(&token)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketMessage {
    #[serde(rename = "type")]
    pub message_type: String,
    pub data: serde_json::Value,
}

pub struct ConnectionStream {
    socket: WebSocketStream<ConnectStream>,
}

impl ConnectionStream {
    pub async fn connect(url: &str) -> Result<Self> {
        let url = url.to_owned();
        let (socket, _) = reqwest_client::runtime()
            .spawn(async move {
                connect_async_with_tls_connector(
                    url,
                    Some(Arc::new(http_client_tls::tls_config()).into()),
                )
                .await
            })
            .await
            .context("WebSocket runtime task failed")?
            .context("failed to connect to Macro connection gateway")?;
        Ok(Self { socket })
    }

    pub async fn send(&mut self, message: &WebSocketMessage) -> Result<()> {
        let json = serde_json::to_string(message).context("failed to serialize WebSocket event")?;
        self.socket
            .send(Message::Text(json.into()))
            .await
            .context("failed to send WebSocket event")
    }

    pub async fn send_text(&mut self, text: &str) -> Result<()> {
        self.socket
            .send(Message::Text(text.to_owned().into()))
            .await
            .context("failed to send WebSocket heartbeat")
    }

    pub async fn recv(&mut self) -> Result<Option<WebSocketMessage>> {
        while let Some(message) = self.socket.next().await {
            match message.context("failed to receive WebSocket event")? {
                Message::Text(text) => {
                    if text == "ping" || text == "pong" {
                        continue;
                    }
                    return serde_json::from_str(text.as_ref())
                        .context("failed to parse WebSocket event")
                        .map(Some);
                }
                Message::Binary(bytes) => {
                    return serde_json::from_slice(bytes.as_ref())
                        .context("failed to parse binary WebSocket event")
                        .map(Some);
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }
}

const SOUP_UPDATES_SUBSCRIPTION_ID: &str = "macro-gpui-soup-updates";
const SOUP_UPDATES_SUBSCRIPTION: &str = r#"
subscription MacroGpuiSoupUpdates {
  soupUpdates {
    __typename
    ... on SoupUpdated {
      item {
        __typename
        id
        entityType
      }
    }
    ... on GraphqlCacheDeletion {
      graphqlTypeName
      entityId
    }
  }
}
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphqlWebSocketMessage {
    #[serde(default)]
    id: Option<String>,
    #[serde(rename = "type")]
    message_type: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
}

/// A `graphql-transport-ws` subscription to Macro's canonical live Soup
/// patches. Callers receive the GraphQL payload rather than transport frames.
pub struct SoupUpdatesStream {
    socket: WebSocketStream<ConnectStream>,
}

impl SoupUpdatesStream {
    pub async fn connect(url: &str) -> Result<Self> {
        let mut request = url
            .into_client_request()
            .context("failed to build Macro Soup websocket request")?;
        request.headers_mut().insert(
            SEC_WEBSOCKET_PROTOCOL,
            "graphql-transport-ws"
                .parse()
                .context("invalid GraphQL websocket subprotocol")?,
        );
        let (socket, _) = reqwest_client::runtime()
            .spawn(async move {
                connect_async_with_tls_connector(
                    request,
                    Some(Arc::new(http_client_tls::tls_config()).into()),
                )
                .await
            })
            .await
            .context("GraphQL websocket runtime task failed")?
            .context("failed to connect to Macro Soup updates")?;
        let mut stream = Self { socket };
        stream
            .send_protocol(GraphqlWebSocketMessage {
                id: None,
                message_type: "connection_init".into(),
                payload: None,
            })
            .await?;

        loop {
            let Some(message) = stream.recv_protocol().await? else {
                anyhow::bail!("Macro Soup websocket closed before connection acknowledgement");
            };
            match message.message_type.as_str() {
                "connection_ack" => break,
                "ping" => {
                    stream
                        .send_protocol(GraphqlWebSocketMessage {
                            id: None,
                            message_type: "pong".into(),
                            payload: message.payload,
                        })
                        .await?;
                }
                "error" | "connection_error" => anyhow::bail!(
                    "Macro Soup websocket rejected the connection: {}",
                    message.payload.unwrap_or(serde_json::Value::Null)
                ),
                _ => {}
            }
        }

        stream
            .send_protocol(GraphqlWebSocketMessage {
                id: Some(SOUP_UPDATES_SUBSCRIPTION_ID.into()),
                message_type: "subscribe".into(),
                payload: Some(serde_json::json!({
                    "operationName": "MacroGpuiSoupUpdates",
                    "query": SOUP_UPDATES_SUBSCRIPTION,
                    "variables": {},
                })),
            })
            .await?;
        Ok(stream)
    }

    async fn send_protocol(&mut self, message: GraphqlWebSocketMessage) -> Result<()> {
        let json = serde_json::to_string(&message)
            .context("failed to serialize GraphQL websocket message")?;
        self.socket
            .send(Message::Text(json.into()))
            .await
            .context("failed to send GraphQL websocket message")
    }

    async fn recv_protocol(&mut self) -> Result<Option<GraphqlWebSocketMessage>> {
        while let Some(message) = self.socket.next().await {
            match message.context("failed to receive GraphQL websocket message")? {
                Message::Text(text) => {
                    return serde_json::from_str(text.as_ref())
                        .context("failed to parse GraphQL websocket message")
                        .map(Some);
                }
                Message::Binary(bytes) => {
                    return serde_json::from_slice(bytes.as_ref())
                        .context("failed to parse binary GraphQL websocket message")
                        .map(Some);
                }
                Message::Close(_) => return Ok(None),
                Message::Ping(payload) => {
                    self.socket
                        .send(Message::Pong(payload))
                        .await
                        .context("failed to answer GraphQL websocket ping")?;
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        Ok(None)
    }

    pub async fn recv(&mut self) -> Result<Option<serde_json::Value>> {
        loop {
            let Some(message) = self.recv_protocol().await? else {
                return Ok(None);
            };
            match message.message_type.as_str() {
                "next" if message.id.as_deref() == Some(SOUP_UPDATES_SUBSCRIPTION_ID) => {
                    let payload = message.payload.unwrap_or(serde_json::Value::Null);
                    if let Some(errors) = payload.get("errors") {
                        anyhow::bail!("Macro Soup subscription returned errors: {errors}");
                    }
                    return Ok(Some(payload));
                }
                "ping" => {
                    self.send_protocol(GraphqlWebSocketMessage {
                        id: None,
                        message_type: "pong".into(),
                        payload: message.payload,
                    })
                    .await?;
                }
                "error" if message.id.as_deref() == Some(SOUP_UPDATES_SUBSCRIPTION_ID) => {
                    anyhow::bail!(
                        "Macro Soup subscription failed: {}",
                        message.payload.unwrap_or(serde_json::Value::Null)
                    );
                }
                "complete" if message.id.as_deref() == Some(SOUP_UPDATES_SUBSCRIPTION_ID) => {
                    return Ok(None);
                }
                _ => {}
            }
        }
    }
}

fn merge_github_pull_requests(
    stored: Vec<GithubPullRequest>,
    enriched: Vec<GithubPullRequest>,
) -> Vec<GithubPullRequest> {
    let mut enriched_by_key = enriched
        .into_iter()
        .map(|pull_request| (pull_request.github_key.clone(), pull_request))
        .collect::<std::collections::HashMap<_, _>>();
    stored
        .into_iter()
        .map(|mut pull_request| {
            let Some(enriched) = enriched_by_key.remove(&pull_request.github_key) else {
                return pull_request;
            };
            pull_request.display_name = enriched.display_name;
            pull_request.number = enriched.number;
            pull_request.owner = enriched.owner;
            pull_request.repo = enriched.repo;
            pull_request.url = enriched.url;
            pull_request.additions = enriched.additions.or(pull_request.additions);
            pull_request.author_login = enriched.author_login.or(pull_request.author_login);
            pull_request.checks = enriched.checks.or(pull_request.checks);
            pull_request.comments = enriched.comments.or(pull_request.comments);
            pull_request.deletions = enriched.deletions.or(pull_request.deletions);
            pull_request.description = enriched.description.or(pull_request.description);
            pull_request.foreign_entity_id = enriched
                .foreign_entity_id
                .or(pull_request.foreign_entity_id);
            pull_request.name = enriched.name.or(pull_request.name);
            pull_request.participant_github_user_ids = enriched
                .participant_github_user_ids
                .or(pull_request.participant_github_user_ids);
            pull_request.status = enriched.status.or(pull_request.status);
            pull_request
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_upload_digest_is_bounded_streamed_and_cancellable() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launch.bin");
        std::fs::write(&path, b"macro").unwrap();
        let control = UploadControl::default();
        let (sha, checksum, size) = bounded_file_digest(&path, 5, &control).unwrap();
        let digest = Sha256::digest(b"macro");
        assert_eq!(sha, format!("{digest:x}"));
        assert_eq!(
            checksum,
            base64::engine::general_purpose::STANDARD.encode(digest)
        );
        assert_eq!(size, 5);
        assert!(bounded_file_digest(&path, 4, &control).is_err());
        assert!(bounded_file_digest(directory.path(), 5, &control).is_err());

        let cancelled = UploadControl::default();
        cancelled.cancel();
        assert!(bounded_file_digest(&path, 5, &cancelled).is_err());

        let oversized = directory.path().join("oversized.bin");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(1_000_001)
            .unwrap();
        assert!(bounded_file_digest(&oversized, 1_000_000, &control).is_err());
    }

    #[test]
    fn isolated_local_stack_proxy_maps_every_public_service_prefix() {
        let config =
            MacroApiConfig::for_local_proxy("http://localhost:31009", "http://localhost:31010")
                .unwrap();
        assert_eq!(config.auth_base_url, "http://localhost:31009/auth");
        assert_eq!(config.storage_base_url, "http://localhost:31009/dss");
        assert_eq!(config.email_base_url, "http://localhost:31009/email");
        assert_eq!(
            config.connection_base_url,
            "http://localhost:31009/connection-gateway"
        );
        assert_eq!(config.sync_http_base_url, "http://localhost:31009/sync");
        assert_eq!(config.sync_websocket_base_url, "ws://localhost:31009/sync");
        assert_eq!(
            config.static_files_base_url,
            "http://localhost:31009/static-file"
        );
        assert_eq!(config.web_app_origin, "http://localhost:31010");
        assert!(
            MacroApiConfig::for_local_proxy("https://macro.com", "http://localhost:3000").is_err()
        );
        assert!(
            MacroApiConfig::for_local_proxy("http://localhost:8090/dss", "http://localhost:3000")
                .is_err()
        );
    }

    #[test]
    fn typed_http_errors_preserve_entitlement_and_retry_metadata() {
        let payment = macro_http_error(HttpTextResponse {
            status: 402,
            text: serde_json::json!({
                "code":"payment_required",
                "message":"chat quota exhausted"
            })
            .to_string(),
            request_id: Some("request-1".into()),
            retry_after: None,
        });
        assert!(payment.is_payment_required());
        assert!(!payment.is_retryable());
        assert_eq!(payment.code.as_deref(), Some("payment_required"));
        assert_eq!(payment.message, "chat quota exhausted");
        assert_eq!(payment.request_id.as_deref(), Some("request-1"));

        let rate_limit = macro_http_error(HttpTextResponse {
            status: 429,
            text: "slow down".into(),
            request_id: None,
            retry_after: Some("10".into()),
        });
        assert!(rate_limit.is_retryable());
        assert_eq!(rate_limit.retry_after.as_deref(), Some("10"));
        assert_eq!(rate_limit.to_string(), "HTTP 429: slow down");
    }

    #[test]
    fn typed_http_errors_preserve_shared_inbox_conflict_details() {
        let conflict = macro_http_error(HttpTextResponse {
            status: 409,
            text: serde_json::json!({
                "email_address": "shared@example.test",
                "existing_owner_email": "owner@example.test",
                "existing_link_id": "link-existing"
            })
            .to_string(),
            request_id: Some("request-conflict".into()),
            retry_after: None,
        });

        assert_eq!(
            conflict.shared_inbox_conflict(),
            Some(SharedInboxConflictResponse {
                email_address: "shared@example.test".into(),
                existing_owner_email: "owner@example.test".into(),
                existing_link_id: "link-existing".into(),
            })
        );
        assert_eq!(conflict.request_id.as_deref(), Some("request-conflict"));
        assert!(conflict.details.is_some());

        let malformed = macro_http_error(HttpTextResponse {
            status: 409,
            text: serde_json::json!({"message":"conflict"}).to_string(),
            request_id: None,
            retry_after: None,
        });
        assert_eq!(malformed.shared_inbox_conflict(), None);
    }

    #[test]
    fn forbidden_errors_follow_macro_web_paywall_policy() {
        let forbidden = macro_http_error(HttpTextResponse {
            status: 403,
            text: serde_json::json!({"message":"premium feature"}).to_string(),
            request_id: None,
            retry_after: None,
        });
        assert!(forbidden.is_payment_required());

        let unauthorized = macro_http_error(HttpTextResponse {
            status: 401,
            text: String::new(),
            request_id: None,
            retry_after: None,
        });
        assert!(unauthorized.is_authentication_required());
        assert!(!unauthorized.is_payment_required());
    }

    #[test]
    fn service_hosts_match_macro_sdk_contract() {
        let local = MacroApiConfig::for_environment(MacroEnvironment::Local);
        assert_eq!(local.auth_base_url, "http://localhost:8080");
        assert_eq!(local.storage_base_url, "http://localhost:8086");
        assert_eq!(local.email_base_url, "http://localhost:8087");
        assert_eq!(local.connection_base_url, "http://localhost:8082");

        let dev = MacroApiConfig::for_environment(MacroEnvironment::Dev);
        assert_eq!(dev.auth_base_url, "https://auth-service-dev.macro.com");
        assert_eq!(dev.storage_base_url, "https://cloud-storage-dev.macro.com");

        let prod = MacroApiConfig::for_environment(MacroEnvironment::Prod);
        assert_eq!(prod.email_base_url, "https://email-service.macro.com");
        assert_eq!(prod.agent_base_url, "https://agent-harness.macro.com");
    }

    #[test]
    fn unified_search_contract_is_typed_and_preserves_future_result_variants() {
        let request = UnifiedSearchRequest {
            query: "quarterly roadmap".into(),
            match_type: UnifiedSearchMatchType::Partial,
            search_on: Some(UnifiedSearchOn::NameContent),
            include_crm: true,
            collapse: None,
            filters: Some(UnifiedSearchEntityFilters {
                document_filters: Some(UnifiedSearchDocumentFilters {
                    owners: vec!["macro|ada@example.test".into()],
                    sub_types: vec!["task".into()],
                    ..Default::default()
                }),
                tag_option_ids: vec!["urgent".into()],
                tag_filter_mode: Some(UnifiedSearchTagFilterMode::All),
                ..Default::default()
            }),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "query":"quarterly roadmap",
                "match_type":"partial",
                "search_on":"name_content",
                "include_crm":true,
                "filters":{
                    "document_filters":{
                        "owners":["macro|ada@example.test"],
                        "sub_types":["task"]
                    },
                    "tag_option_ids":["urgent"],
                    "tag_filter_mode":"all"
                }
            })
        );

        let response: UnifiedSearchResponse = serde_json::from_value(serde_json::json!({
            "results":[
                {"type":"document","id":"doc-1","future_rank":7},
                {"type":"whiteboard","id":"board-1","future_field":{"kept":true}}
            ],
            "next_cursor":"page-2"
        }))
        .unwrap();
        assert_eq!(
            response.results[0].kind(),
            &UnifiedSearchResultKind::Document
        );
        assert_eq!(
            response.results[1].kind(),
            &UnifiedSearchResultKind::Unknown("whiteboard".into())
        );
        assert_eq!(response.results[1].raw()["future_field"]["kept"], true);
        assert_eq!(
            serde_json::to_value(&response).unwrap()["results"][1]["type"],
            "whiteboard"
        );
    }

    #[test]
    fn websocket_url_matches_macro_frontend_and_encodes_token() {
        let api = MacroApi::new(MacroApiConfig::default());
        api.set_token("token+/=".into());
        assert_eq!(
            api.websocket_url(),
            "ws://localhost:8082/?macro-api-token=token%2B%2F%3D"
        );
        assert_eq!(
            api.soup_websocket_url(),
            "ws://localhost:8086/items/soup/graphql/ws?macro-api-token=token%2B%2F%3D"
        );
    }

    #[test]
    fn soup_subscription_uses_macro_graphql_transport_and_deletion_contract() {
        assert!(SOUP_UPDATES_SUBSCRIPTION.contains("subscription MacroGpuiSoupUpdates"));
        assert!(SOUP_UPDATES_SUBSCRIPTION.contains("GraphqlCacheDeletion"));
        assert!(SOUP_UPDATES_SUBSCRIPTION.contains("graphqlTypeName"));
        assert!(SOUP_UPDATES_SUBSCRIPTION.contains("entityType"));
    }

    #[test]
    fn tag_contract_accepts_list_and_mutation_response_casing() {
        let list: TagSetResponse = serde_json::from_value(serde_json::json!({
            "scope": "user",
            "definition": {
                "id": "definition-1",
                "displayName": "Tags",
                "dataType": "TAG",
                "isMultiSelect": true
            },
            "options": [{
                "id": "tag-1",
                "propertyDefinitionId": "definition-1",
                "displayOrder": 2,
                "value": { "type": "string", "value": "Important" },
                "color": "#ff0000"
            }]
        }))
        .unwrap();
        assert_eq!(list.options[0].label(), "Important");

        let mutation: TagOption = serde_json::from_value(serde_json::json!({
            "id": "tag-2",
            "property_definition_id": "definition-1",
            "display_order": 3,
            "value": { "type": "string", "value": "Follow up" },
            "color": null
        }))
        .unwrap();
        assert_eq!(mutation.display_order, 3);

        let request = AddStringPropertyOptionRequest {
            option_type: "select_string".into(),
            option: AddStringPropertyOption {
                value: "Important".into(),
                color: Some("#ff0000".into()),
                display_order: 0,
            },
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["type"], "select_string");
        assert_eq!(value["option"]["display_order"], 0);
    }

    #[test]
    fn property_definition_contract_matches_generated_properties_sdk() {
        let definition: PropertyDefinitionResponse = serde_json::from_value(serde_json::json!({
            "definition": {
                "created_at":"2026-08-27T12:00:00Z",
                "data_type":"SELECT_STRING",
                "display_name":"Impact",
                "id":"definition-1",
                "is_metadata":false,
                "is_multi_select":true,
                "is_system":false,
                "owner":{"scope":"team","team_id":"team-1"},
                "specific_entity_type":"DOCUMENT",
                "updated_at":"2026-08-27T12:00:00Z"
            },
            "property_options":[]
        }))
        .unwrap();
        assert_eq!(definition.definition().display_name, "Impact");
        assert!(matches!(
            definition.definition().owner,
            PropertyOwner::Team { .. }
        ));

        let request = CreatePropertyDefinitionRequest {
            data_type: CreatePropertyDataType::SelectString {
                multi: true,
                options: vec![SelectStringOption {
                    display_order: 0,
                    value: "High".into(),
                }],
            },
            display_name: "Impact".into(),
            scope: "team".into(),
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "data_type":{
                    "type":"select_string",
                    "multi":true,
                    "options":[{"display_order":0,"value":"High"}]
                },
                "display_name":"Impact",
                "scope":"team"
            })
        );
    }

    #[test]
    fn entity_mention_contract_preserves_generated_snake_case_fields() {
        assert_eq!(
            serde_json::to_value(CreateEntityMentionRequest {
                entity_id: "task-1".into(),
                entity_type: "document".into(),
                source_entity_id: "doc-1".into(),
                source_entity_type: "document".into(),
            })
            .unwrap(),
            serde_json::json!({
                "entity_id":"task-1",
                "entity_type":"document",
                "source_entity_id":"doc-1",
                "source_entity_type":"document"
            })
        );

        let response: CreateEntityMentionResponse = serde_json::from_value(serde_json::json!({
            "created_at":"2026-08-28T12:00:00Z",
            "entity_id":"task-1",
            "entity_type":"document",
            "id":"mention-1",
            "source_entity_id":"doc-1",
            "source_entity_type":"document",
            "user_id":"user-1"
        }))
        .unwrap();
        assert_eq!(response.id, "mention-1");
        assert_eq!(response.user_id.as_deref(), Some("user-1"));
    }

    #[test]
    fn sharing_contract_distinguishes_omission_from_explicit_null() {
        let clear_link = UpdateSharePermissionRequestV2 {
            channel_share_permissions: Some(vec![UpdateChannelSharePermission {
                access_level: Some(ShareAccessLevel::Comment),
                channel_id: "channel-1".into(),
                operation: ShareUpdateOperation::Add,
            }]),
            link_share: Some(None),
            link_share_access_level: Some(None),
        };
        assert_eq!(
            serde_json::to_value(clear_link).unwrap(),
            serde_json::json!({
                "channelSharePermissions":[{
                    "accessLevel":"comment",
                    "channelId":"channel-1",
                    "operation":"add"
                }],
                "linkShare":null,
                "linkShareAccessLevel":null
            })
        );
        assert_eq!(
            serde_json::to_value(UpdateSharePermissionRequestV2::default()).unwrap(),
            serde_json::json!({})
        );

        let policy: SharePermissionV2 = serde_json::from_value(serde_json::json!({
            "channelSharePermissions":[{
                "access_level":"edit",
                "channel_id":"channel-2"
            }],
            "id":"share-1",
            "linkShare":"TEAM",
            "linkShareAccessLevel":"view",
            "owner":"user-1"
        }))
        .unwrap();
        assert_eq!(policy.link_share, Some(LinkShare::Team));
        assert_eq!(
            policy.channel_share_permissions.unwrap()[0].access_level,
            ShareAccessLevel::Edit
        );

        let access: GetEntityPermissionResponse = serde_json::from_value(serde_json::json!({
            "status":"access",
            "permission":{"type":"channel_role","role":"admin"}
        }))
        .unwrap();
        assert_eq!(
            access,
            GetEntityPermissionResponse::Access {
                permission: ApiEntityPermission::ChannelRole {
                    role: EntityMemberRole::Admin
                }
            }
        );
        let no_access: GetEntityPermissionResponse =
            serde_json::from_value(serde_json::json!({"status":"no_access"})).unwrap();
        assert_eq!(no_access, GetEntityPermissionResponse::NoAccess);
    }

    #[test]
    fn people_cache_contracts_match_auth_and_contacts_sdks() {
        assert_eq!(
            serde_json::to_value(UserIdsRequest {
                user_ids: vec!["macro|ada@example.com".into()]
            })
            .unwrap(),
            serde_json::json!({"user_ids":["macro|ada@example.com"]})
        );
        let names: UserNamesResponse = serde_json::from_value(serde_json::json!({
            "names":[{
                "first_name":"Ada",
                "id":"macro|ada@example.com",
                "last_name":"Lovelace"
            }]
        }))
        .unwrap();
        assert_eq!(names.names[0].last_name.as_deref(), Some("Lovelace"));
        let contacts: GetContactsResponse = serde_json::from_value(serde_json::json!({
            "contacts":["macro|grace@example.com"]
        }))
        .unwrap();
        assert_eq!(contacts.contacts.len(), 1);
        assert_eq!(
            serde_json::to_value(AddContactRequest {
                user_id: "macro|grace@example.com".into()
            })
            .unwrap(),
            serde_json::json!({"user_id":"macro|grace@example.com"})
        );
    }

    #[test]
    fn document_comment_contract_uses_generated_camel_case_shape() {
        let response: DocumentCommentsResponse = serde_json::from_value(serde_json::json!({
            "data": [{
                "thread": {
                    "createdAt": "2026-08-27T12:00:00Z",
                    "deletedAt": null,
                    "documentId": "doc-1",
                    "owner": "macro|owner",
                    "resolved": false,
                    "threadId": 7,
                    "updatedAt": null
                },
                "comments": [{
                    "commentId": 9,
                    "createdAt": "2026-08-27T12:00:00Z",
                    "deletedAt": null,
                    "order": 0,
                    "owner": "macro|owner",
                    "sender": "Owner",
                    "text": "Review this",
                    "threadId": 7,
                    "updatedAt": null
                }]
            }]
        }))
        .unwrap();
        assert_eq!(response.data[0].thread.thread_id, 7);
        assert_eq!(response.data[0].comments[0].comment_id, 9);
        assert_eq!(
            serde_json::to_value(CreateDocumentCommentRequest {
                text: "Reply".into(),
                mentions: None,
                thread_id: Some(7),
                metadata: None,
                thread_metadata: None,
            })
            .unwrap(),
            serde_json::json!({"text":"Reply","threadId":7})
        );
        assert_eq!(
            serde_json::to_value(DeleteDocumentCommentRequest {
                remove_anchor_thread_only: false
            })
            .unwrap(),
            serde_json::json!({"removeAnchorThreadOnly":false})
        );
        let deleted: DeleteDocumentCommentResponse = serde_json::from_value(serde_json::json!({
            "anchor": null,
            "commentId": 9,
            "documentId": "doc-1",
            "thread": {"deleted": true, "threadId": 7}
        }))
        .unwrap();
        assert!(deleted.thread.deleted);
        assert_eq!(deleted.thread.thread_id, 7);
    }

    #[test]
    fn task_request_uses_storage_openapi_field_names() {
        let value = serde_json::to_value(CreateTaskRequest {
            task_name: "Review the contract",
            project_id: Some("project-1"),
            team_id: None,
            markdown: None,
            property_values: Some(&[
                TaskPropertyInput {
                    property_id: "00000001-0000-0000-0000-000000000002".into(),
                    value: SetPropertyValue::SelectOption {
                        option_id: "00000001-0000-0000-0002-000000000001".into(),
                    },
                },
                TaskPropertyInput {
                    property_id: "00000001-0000-0000-0000-000000000001".into(),
                    value: SetPropertyValue::MultiEntityReference {
                        references: vec![PropertyEntityReference {
                            entity_id: "user-1".into(),
                            entity_type: "USER".into(),
                            specific_message_id: None,
                        }],
                    },
                },
            ]),
            share_with_team: true,
        })
        .unwrap();

        assert_eq!(value["taskName"], "Review the contract");
        assert_eq!(value["projectId"], "project-1");
        assert_eq!(value["shareWithTeam"], true);
        assert_eq!(value["propertyValues"][0]["value"]["type"], "select_option");
        assert_eq!(
            value["propertyValues"][1]["value"]["references"][0]["entity_type"],
            "USER"
        );
        assert!(value.get("task_name").is_none());

        let bulk_tags = BulkUpdateEntitiesPropertyOptionsRequest {
            add_option_ids: vec!["00000001-0000-0000-0009-000000000001".into()],
            entities: vec![
                PropertyTargetReference {
                    entity_id: "task-1".into(),
                    entity_type: "DOCUMENT".into(),
                },
                PropertyTargetReference {
                    entity_id: "task-2".into(),
                    entity_type: "DOCUMENT".into(),
                },
            ],
            property_id: "00000001-0000-0000-0000-000000000009".into(),
            remove_option_ids: Vec::new(),
        };
        assert_eq!(
            serde_json::to_value(bulk_tags).unwrap(),
            serde_json::json!({
                "add_option_ids":["00000001-0000-0000-0009-000000000001"],
                "entities":[
                    {"entity_id":"task-1","entity_type":"DOCUMENT"},
                    {"entity_id":"task-2","entity_type":"DOCUMENT"}
                ],
                "property_id":"00000001-0000-0000-0000-000000000009"
            })
        );
        let bulk_response: BulkUpdateEntitiesPropertyOptionsResponse =
            serde_json::from_value(serde_json::json!({"results":[
                {
                    "entity_id":"task-1",
                    "entity_type":"DOCUMENT",
                    "option_ids":["00000001-0000-0000-0009-000000000001"],
                    "status":"applied"
                },
                {
                    "entity_id":"task-2",
                    "entity_type":"DOCUMENT",
                    "error":"permission denied",
                    "status":"skipped_no_permission"
                }
            ]}))
            .unwrap();
        assert_eq!(bulk_response.results[0].status, "applied");
        assert_eq!(
            bulk_response.results[1].error.as_deref(),
            Some("permission denied")
        );

        assert_eq!(
            serde_json::to_value(CreateSnippetRequest {
                markdown: Some("Hello **world**".into()),
                project_id: Some("project-1".into()),
                snippet_name: "Greeting".into(),
            })
            .unwrap(),
            serde_json::json!({
                "markdown":"Hello **world**",
                "projectId":"project-1",
                "snippetName":"Greeting"
            })
        );

        assert_eq!(
            serde_json::to_value(CopyDocumentRequest {
                document_name: "Review the contract copy".into(),
                version_id: None,
            })
            .unwrap(),
            serde_json::json!({"documentName":"Review the contract copy"})
        );
        assert_eq!(
            serde_json::to_value(CopyDocumentRequest {
                document_name: "Historical copy".into(),
                version_id: Some(SyncServiceVersionId {
                    counter: 42,
                    peer: "9007199254740991".into(),
                }),
            })
            .unwrap(),
            serde_json::json!({
                "documentName":"Historical copy",
                "versionId":{"counter":42,"peer":"9007199254740991"}
            })
        );
        assert_eq!(
            serde_json::to_value(EditDocumentRequest {
                project_id: Some(Some("project-2".into())),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({"projectId":"project-2"})
        );
        assert_eq!(
            serde_json::to_value(EditDocumentRequest {
                project_id: Some(None),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({"projectId":null})
        );
        assert_eq!(
            serde_json::to_value(SetDocumentTeamShareRequest {
                share_with_team: true,
            })
            .unwrap(),
            serde_json::json!({"shareWithTeam":true})
        );
        assert_eq!(
            serde_json::to_value(CreateBinaryDocumentRequest {
                document_name: "report.pdf".into(),
                sha: "abc".into(),
                file_type: Some("pdf".into()),
                mime_type: Some("application/pdf".into()),
                project_id: None,
            })
            .unwrap(),
            serde_json::json!({
                "documentName":"report.pdf",
                "sha":"abc",
                "fileType":"pdf",
                "mimeType":"application/pdf"
            })
        );
        let ready: DocumentLocationResponse = serde_json::from_value(serde_json::json!({
            "content": {"state":"ready", "location":"object_storage"},
            "metadata": {"documentId":"doc-1", "documentVersionId":2},
            "presignedUrl":"https://example.invalid/file",
            "type":"presignedUrl"
        }))
        .unwrap();
        assert!(ready.content_is_ready());
        assert_eq!(ready.location_type, "presignedUrl");
        let pending: DocumentLocationResponse = serde_json::from_value(serde_json::json!({
            "content": {"state":"pending"},
            "metadata": {},
            "type":"syncServiceContent"
        }))
        .unwrap();
        assert!(!pending.content_is_ready());
        assert_eq!(
            serde_json::to_value(UploadExtractFolderRequest {
                name: Some("research".into()),
                parent_id: Some("project-1".into()),
                sha: "abc123".into(),
            })
            .unwrap(),
            serde_json::json!({
                "name":"research",
                "parentId":"project-1",
                "sha":"abc123"
            })
        );
    }

    #[test]
    fn email_read_and_send_contracts_match_generated_service_shapes() {
        let response: GetEmailThreadResponse = serde_json::from_value(serde_json::json!({
            "thread": {
                "access_level": "owner",
                "created_at": "2026-08-27T12:00:00Z",
                "db_id": "thread-1",
                "inbox_visible": true,
                "is_read": false,
                "latest_inbound_message_ts": "2026-08-27T12:00:00Z",
                "link_id": "link-1",
                "messages": [{
                    "attachments": [],
                    "attachments_draft": [],
                    "attachments_forwarded": [],
                    "bcc": [],
                    "body_replyless": "Hello Alex",
                    "body_text": "Hello Alex",
                    "cc": [],
                    "created_at": "2026-08-27T12:00:00Z",
                    "db_id": "message-1",
                    "from": {"email":"person@example.com","name":"Person"},
                    "has_attachments": false,
                    "is_draft": false,
                    "is_read": false,
                    "is_sent": false,
                    "is_starred": true,
                    "labels": [{
                        "created_at":"2026-08-27T12:00:00Z",
                        "link_id":"link-1",
                        "provider_label_id":"STARRED",
                        "type_":"System"
                    }],
                    "link_id": "link-1",
                    "subject": "Contract",
                    "thread_db_id": "thread-1",
                    "to": [{"email":"alex@example.com"}],
                    "updated_at": "2026-08-27T12:00:00Z",
                    "future_message_field": {"preserved":true}
                }],
                "updated_at": "2026-08-27T12:00:00Z",
                "future_thread_field": "preserved"
            }
        }))
        .unwrap();
        assert_eq!(response.thread.access_level, EmailAccessLevel::Owner);
        assert_eq!(response.thread.messages[0].db_id, "message-1");
        assert_eq!(
            response.thread.messages[0].from.as_ref().unwrap().email,
            "person@example.com"
        );
        let round_trip = serde_json::to_value(response).unwrap();
        assert_eq!(round_trip["thread"]["future_thread_field"], "preserved");
        assert_eq!(
            round_trip["thread"]["messages"][0]["future_message_field"]["preserved"],
            true
        );

        let parsed: Vec<ParsedEmailMessage> = serde_json::from_value(serde_json::json!([{
            "bcc": [],
            "body_parsed": "Hello Alex",
            "cc": [],
            "db_id": "message-1",
            "from": {"email":"person@example.com"},
            "labels": [{"name":"Inbox","provider_id":"INBOX"}],
            "link_id": "link-1",
            "thread_db_id": "thread-1",
            "to": [{"email":"alex@example.com"}]
        }]))
        .unwrap();
        assert_eq!(parsed[0].labels[0].provider_id, "INBOX");

        let sent: SendEmailMessageResponse = serde_json::from_value(serde_json::json!({
            "message": {
                "db_id": "message-2",
                "link_id": "link-1",
                "subject": "Re: Contract",
                "thread_db_id": "thread-1",
                "to": [{"email":"person@example.com","photo_url":"https://example.test/p.png"}]
            }
        }))
        .unwrap();
        assert_eq!(sent.message.db_id.as_deref(), Some("message-2"));
        assert_eq!(
            sent.message.to[0].photo_url.as_deref(),
            Some("https://example.test/p.png")
        );

        let attachment: GetEmailAttachmentDocumentIdResponse =
            serde_json::from_value(serde_json::json!({
                "attachment_id":"attachment-1",
                "document_id":"document-1"
            }))
            .unwrap();
        assert_eq!(attachment.document_id, "document-1");

        let links: ListEmailLinksResponse = serde_json::from_value(serde_json::json!({
            "links": [{
                "calendar_disabled": false,
                "created_at": "2026-08-27T12:00:00Z",
                "email_address": "alex@example.com",
                "fusionauth_user_id": "fusion-user",
                "has_calendar_data": true,
                "id": "link-1",
                "is_primary": true,
                "is_sync_active": true,
                "macro_id": "macro-user",
                "needs_calendar_permission": false,
                "needs_reauth": false,
                "provider": "GMAIL",
                "settings": {
                    "signature": "Alex",
                    "signature_on_replies_forwards": true
                },
                "sync_status": "UP_TO_DATE",
                "updated_at": "2026-08-27T12:00:00Z",
                "future_link_field": "preserved"
            }]
        }))
        .unwrap();
        assert_eq!(links.links[0].sync_status, EmailSyncStatus::UpToDate);
        assert_eq!(links.links[0].sync_status.label(), "up to date");
        assert_eq!(
            serde_json::to_value(links).unwrap()["links"][0]["future_link_field"],
            "preserved"
        );
    }

    #[test]
    fn reply_request_preserves_thread_correlation_fields() {
        let value = serde_json::to_value(SendMessageRequest {
            message: EmailDraftInput {
                subject: "Re: Contract".into(),
                to: vec![EmailContactInput {
                    email: "person@example.com".into(),
                    name: None,
                    photo_url: None,
                }],
                cc: Vec::new(),
                bcc: Vec::new(),
                thread_db_id: Some("thread-1".into()),
                replying_to_id: Some("message-9".into()),
                body_text: Some("Approved".into()),
                body_html: None,
                ..Default::default()
            },
        })
        .unwrap();

        assert_eq!(value["message"]["thread_db_id"], "thread-1");
        assert_eq!(value["message"]["replying_to_id"], "message-9");
        assert_eq!(value["message"]["to"][0]["email"], "person@example.com");
        assert!(value["message"].get("cc").is_none());
    }

    #[test]
    fn email_draft_schedule_attachment_and_settings_match_generated_contracts() {
        let draft = serde_json::to_value(CreateEmailDraftRequest {
            draft: EmailDraftInput {
                subject: "Quarterly plan".into(),
                to: vec![EmailContactInput {
                    email: "to@example.com".into(),
                    name: Some("To".into()),
                    photo_url: None,
                }],
                cc: vec![EmailContactInput {
                    email: "cc@example.com".into(),
                    name: None,
                    photo_url: None,
                }],
                bcc: Vec::new(),
                body_text: Some("Review attached".into()),
                include_signature: Some(true),
                ..Default::default()
            },
            send_time: Some("2026-08-28T13:00:00Z".into()),
        })
        .unwrap();
        assert_eq!(draft["draft"]["to"][0]["email"], "to@example.com");
        assert_eq!(draft["draft"]["cc"][0]["email"], "cc@example.com");
        assert_eq!(draft["draft"]["include_signature"], true);
        assert_eq!(draft["send_time"], "2026-08-28T13:00:00Z");
        assert!(draft["draft"].get("bcc").is_none());

        assert_eq!(
            serde_json::to_value(ScheduleEmailDraftRequest {
                send_time: "2026-08-28T13:00:00Z".into()
            })
            .unwrap(),
            serde_json::json!({"send_time":"2026-08-28T13:00:00Z"})
        );
        assert_eq!(
            serde_json::to_value(AddEmailDraftAttachmentRequest {
                file_name: "plan.pdf".into(),
                sha: "abc123".into(),
                size: 42,
            })
            .unwrap(),
            serde_json::json!({"file_name":"plan.pdf","sha":"abc123","size":42})
        );
        assert_eq!(
            serde_json::to_value(AddForwardedEmailAttachmentRequest {
                attachment_id: "source-attachment-1".into(),
            })
            .unwrap(),
            serde_json::json!({"attachment_id":"source-attachment-1"})
        );
        assert_eq!(
            serde_json::to_value(PatchEmailSettingsRequest {
                settings: EmailSettings {
                    signature: Some("<p>Nick</p>".into()),
                    signature_on_replies_forwards: Some(true),
                }
            })
            .unwrap(),
            serde_json::json!({
                "settings": {
                    "signature":"<p>Nick</p>",
                    "signature_on_replies_forwards":true
                }
            })
        );

        let scheduled: ScheduledEmailDraftsResponse = serde_json::from_value(serde_json::json!({
            "messages": [{
                "db_id":"draft-1",
                "thread_db_id":"thread-1",
                "link_id":"link-1",
                "subject":"Quarterly plan",
                "to":[{"email":"to@example.com","name":"To"}],
                "cc":[],
                "bcc":[],
                "scheduled_send_time":"2026-08-28T13:00:00Z",
                "created_at":"2026-08-28T12:00:00Z",
                "updated_at":"2026-08-28T12:01:00Z",
                "attachments":[],
                "labels":[]
            }]
        }))
        .unwrap();
        assert_eq!(scheduled.messages[0].db_id, "draft-1");
        assert_eq!(scheduled.messages[0].to[0].email, "to@example.com");
        assert_eq!(
            scheduled.messages[0].scheduled_send_time.as_deref(),
            Some("2026-08-28T13:00:00Z")
        );

        let attachment = EmailAttachment {
            content_id: None,
            data_url: Some("data:text/plain;base64,SGVsbG8sIE1hY3JvIQ==".into()),
            db_id: "attachment-1".into(),
            filename: Some("hello.txt".into()),
            mime_type: Some("text/plain".into()),
            provider_id: None,
            sfs_id: None,
            size_bytes: Some(13),
        };
        assert_eq!(
            attachment.decoded_data().unwrap(),
            Some(b"Hello, Macro!".to_vec())
        );
        let percent_encoded = EmailAttachment {
            data_url: Some("data:text/plain,hello%20world".into()),
            size_bytes: Some(11),
            ..attachment
        };
        assert_eq!(
            percent_encoded.decoded_data().unwrap(),
            Some(b"hello world".to_vec())
        );
    }

    #[test]
    fn active_calls_follow_storage_camel_case_schema() {
        let response: ActiveCallsResponse = serde_json::from_value(serde_json::json!({
            "calls": [{
                "callId": "call-1",
                "channelId": "channel-1",
                "createdAt": "2026-08-27T12:00:00Z",
                "createdBy": "user-1",
                "participantCount": 2
            }]
        }))
        .unwrap();

        assert_eq!(response.calls[0].channel_id, "channel-1");
        assert_eq!(response.calls[0].participant_count, 2);

        let active: CallActiveResponse = serde_json::from_value(serde_json::json!({
            "callId":"call-1",
            "channelId":"channel-1",
            "createdAt":"2026-08-27T12:00:00Z",
            "createdBy":"user-1"
        }))
        .unwrap();
        assert_eq!(active.call_id, "call-1");
        assert_eq!(
            serde_json::from_value::<RingStatusResponse>(serde_json::json!({
                "status":"answered"
            }))
            .unwrap()
            .status,
            RingStatus::Answered
        );
        assert_eq!(
            serde_json::to_value(LeaveCallResponse { call_ended: true }).unwrap(),
            serde_json::json!({"callEnded":true})
        );

        let record: CallRecord = serde_json::from_value(serde_json::json!({
            "callId":"call-1",
            "channelId":"channel-1",
            "channelName":"Design",
            "createdBy":"user-1",
            "durationMs":120000,
            "isActive":false,
            "participants":[{
                "joinedAt":"2026-08-27T12:00:00Z",
                "leftAt":"2026-08-27T12:02:00Z",
                "userId":"user-1"
            }],
            "recordingUrl":"https://example.com/recording.mp4",
            "roomName":"room-1",
            "shareWithTeam":true,
            "startedAt":"2026-08-27T12:00:00Z",
            "status":"ATTENDED",
            "summary":"Reviewed the design",
            "transcript":[{
                "content":"Looks good",
                "sequenceNum":1,
                "speakerId":"user-1",
                "startedAt":"2026-08-27T12:00:10Z",
                "transcriptId":"transcript-1"
            }]
        }))
        .unwrap();
        assert_eq!(record.status, Some(CallStatus::Attended));
        assert_eq!(record.transcript[0].content, "Looks good");
        assert_eq!(
            serde_json::to_value(EditCallRecordRequest {
                custom_name: Some("Design review".into()),
                share_with_team: None,
            })
            .unwrap(),
            serde_json::json!({"customName":"Design review"})
        );
        assert_eq!(
            serde_json::to_value(EditCallTranscriptRequest {
                assignments: vec![CustomSpeakerAssignment {
                    custom_speaker: Some("macro|nick@example.com".into()),
                    diarized_speaker_id: "speaker_0".into(),
                }],
            })
            .unwrap(),
            serde_json::json!({
                "assignments":[{
                    "customSpeaker":"macro|nick@example.com",
                    "diarizedSpeakerId":"speaker_0"
                }]
            })
        );
    }

    #[test]
    fn agent_contract_matches_generated_harness_schema() {
        let create = serde_json::to_value(CreateAgentSessionRequest {
            bot_id: None,
            owner: None,
            prompt: Some("Review this repository".into()),
            repo_url: Some("https://github.com/macro-inc/macro".into()),
            thread: None,
            workspace: None,
        })
        .unwrap();
        assert_eq!(create["prompt"], "Review this repository");
        assert_eq!(create["repoUrl"], "https://github.com/macro-inc/macro");
        assert!(create.get("botId").is_none());

        assert_eq!(
            serde_json::to_value(AgentAction::SetModel {
                model: "claude-sonnet-4-5".into()
            })
            .unwrap(),
            serde_json::json!({"type":"setModel","model":"claude-sonnet-4-5"})
        );
        assert_eq!(
            serde_json::to_value(AgentAction::Compact).unwrap(),
            serde_json::json!({"type":"compact"})
        );
        assert_eq!(
            serde_json::to_value(SandboxSizeBody {
                size: SandboxSize::Large
            })
            .unwrap(),
            serde_json::json!({"size":"large"})
        );

        let session: AgentSessionResponse = serde_json::from_value(serde_json::json!({
            "botId":"bot-1",
            "createdAt":"2026-08-27T12:00:00Z",
            "harness":"claude-code",
            "id":"session-1",
            "model":"claude-sonnet-4-5",
            "modifiedAt":"2026-08-27T12:01:00Z",
            "name":"Review",
            "ownerId":"macro|owner",
            "sandboxSize":"default",
            "status":{"kind":"event","event":"acp_ready"},
            "workspace":"/workspace"
        }))
        .unwrap();
        assert_eq!(session.sandbox_size, SandboxSize::Default);
        assert_eq!(
            session.status,
            AgentSessionStatus::Event {
                event: "acp_ready".into()
            }
        );
        assert_eq!(session.status.label(), "Ready");
        assert!(!session.status.can_resume());
        assert_eq!(AgentSessionStatus::Disconnected.label(), "Disconnected");
        assert!(AgentSessionStatus::Disconnected.can_resume());
    }

    #[test]
    fn agent_log_fold_matches_macro_turn_tool_permission_and_plan_contracts() {
        let entries = vec![
            (AgentLogDirection::ToServer, serde_json::json!({"type":"event","event":"acp_ready"})),
            (AgentLogDirection::ToRuntime, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","id":"agent_session:prompt-1",
                "method":"session/prompt","params":{"sessionId":"s1","prompt":[{"type":"text","text":"Ship it"}]}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Inspecting"}
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"tool_call","toolCallId":"run-1","kind":"execute","status":"pending","title":"Run tests",
                    "rawInput":{"command":"cargo test"},"content":[{"type":"terminal","terminalId":"run-1"}],
                    "_meta":{"claudeCode":{"toolName":"Bash"}}
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","id":"permission-1","method":"session/request_permission","params":{
                    "sessionId":"s1","toolCall":{"toolCallId":"run-1"},"options":[
                        {"kind":"reject_once","name":"Deny","optionId":"reject"},
                        {"kind":"allow_once","name":"Allow once","optionId":"allow"}
                    ]
                }
            })),
            (AgentLogDirection::ToRuntime, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","id":"permission-1","result":{"outcome":{"outcome":"selected","optionId":"allow"}}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"tool_call_update","toolCallId":"run-1","status":"completed",
                    "_meta":{"terminal_output":{"terminal_id":"run-1","data":"ok\n"},"terminal_exit":{"terminal_id":"run-1","exit_code":0}}
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"tool_call","toolCallId":"edit-1","kind":"edit","status":"completed","title":"Write",
                    "rawInput":{"filePath":"src/main.rs","content":"fn main() {}"},"content":[]
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"plan","entries":[
                        {"content":"Inspect","priority":"high","status":"completed"},
                        {"content":"Verify","priority":"medium","status":"in_progress"}
                    ]
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                    "sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Done."}
                }}
            })),
            (AgentLogDirection::ToServer, serde_json::json!({
                "type":"acp","jsonrpc":"2.0","id":"agent_session:prompt-1","result":{"stopReason":"end_turn"}
            })),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (direction, content))| AgentSessionLogEntry {
            content,
            direction,
            created_at: format!("2026-08-28T12:00:{index:02}Z"),
            user_id: (direction == AgentLogDirection::ToRuntime).then(|| "macro|owner".into()),
        })
        .collect();
        let transcript = fold_agent_session_log(
            "00000000-0000-0000-0000-000000000001",
            &AgentSessionLogResponse {
                bot: AgentSessionBot {
                    avatar_url: None,
                    id: "bot-1".into(),
                    name: "Macro".into(),
                },
                entries,
            },
        )
        .unwrap();

        assert_eq!(transcript.metadata.status.as_deref(), Some("acp_ready"));
        assert_eq!(transcript.messages.len(), 2);
        assert!(transcript.messages[0].author.is_user());
        assert_eq!(
            transcript.messages[0].request_id.as_deref(),
            Some("agent_session:prompt-1")
        );
        let agent = &transcript.messages[1];
        assert_eq!(agent.stop, Some(AgentStopReason::EndTurn));
        assert!(
            matches!(&agent.parts[0], AgentTranscriptPart::Thought { text } if text == "Inspecting")
        );
        assert!(matches!(&agent.parts[1], AgentTranscriptPart::ToolUse {
            label, status: AgentToolStatus::Completed,
            detail: AgentToolDetail::Terminal { command: Some(command), output: Some(output), exit_code: Some(0) }, ..
        } if label == "Bash" && command == "cargo test" && output == "ok\n"));
        assert!(matches!(&agent.parts[2], AgentTranscriptPart::Permission {
            outcome: AgentPermissionOutcome::Selected { option_id }, ..
        } if option_id == "allow"));
        assert!(matches!(&agent.parts[3], AgentTranscriptPart::ToolUse {
            detail: AgentToolDetail::Edit { diffs }, ..
        } if diffs.len() == 1 && diffs[0].path == "src/main.rs" && diffs[0].new_text == "fn main() {}"));
        assert!(
            matches!(&agent.parts[4], AgentTranscriptPart::Plan { entries }
            if entries.len() == 2 && entries[1].status == AgentPlanStatus::InProgress)
        );
        assert!(matches!(&agent.parts[5], AgentTranscriptPart::Text { text } if text == "Done."));
    }

    #[test]
    fn agent_log_fold_preserves_prompt_message_and_tool_artifacts() {
        let frames = vec![
            (
                AgentLogDirection::ToRuntime,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","id":"agent_session:artifact-prompt",
                    "method":"session/prompt","params":{"sessionId":"s1","prompt":[
                        {"type":"text","text":"Review these"},
                        {"type":"resource_link","name":"main.rs","title":"Source", "description":"Entrypoint",
                         "uri":"file:///workspace/src/main.rs","mimeType":"text/rust","size":128},
                        {"type":"resource","resource":{"uri":"macro://context","mimeType":"text/markdown","text":"# Context"}}
                    ]}
                }),
            ),
            (
                AgentLogDirection::ToServer,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                        "sessionUpdate":"agent_message_chunk","content":{"type":"image","data":"aW1hZ2U=","mimeType":"image/png","uri":"file:///preview.png"}
                    }}
                }),
            ),
            (
                AgentLogDirection::ToServer,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                        "sessionUpdate":"agent_message_chunk","content":{"type":"audio","data":"YXVkaW8=","mimeType":"audio/wav"}
                    }}
                }),
            ),
            (
                AgentLogDirection::ToServer,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                        "sessionUpdate":"tool_call","toolCallId":"fetch-1","kind":"fetch","status":"running","title":"Fetch",
                        "content":[{"type":"content","content":{"type":"resource_link","name":"Result","uri":"https://example.com/result"}}]
                    }}
                }),
            ),
            (
                AgentLogDirection::ToServer,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{
                        "sessionUpdate":"tool_call_update","toolCallId":"fetch-1","status":"completed","content":[
                            {"type":"content","content":{"type":"text","text":"Fetched"}},
                            {"type":"content","content":{"type":"resource","resource":{"uri":"tool://archive.bin","mimeType":"application/octet-stream","blob":"YmluYXJ5"}}}
                        ]
                    }}
                }),
            ),
            (
                AgentLogDirection::ToServer,
                serde_json::json!({
                    "type":"acp","jsonrpc":"2.0","id":"agent_session:artifact-prompt","result":{"stopReason":"end_turn"}
                }),
            ),
        ];
        let entries = frames
            .into_iter()
            .enumerate()
            .map(|(index, (direction, content))| AgentSessionLogEntry {
                content,
                direction,
                created_at: format!("2026-08-28T13:00:{index:02}Z"),
                user_id: None,
            })
            .collect();
        let transcript = fold_agent_session_log(
            "session-1",
            &AgentSessionLogResponse {
                bot: AgentSessionBot {
                    avatar_url: None,
                    id: "bot-1".into(),
                    name: "Macro".into(),
                },
                entries,
            },
        )
        .unwrap();

        let user = &transcript.messages[0];
        assert_eq!(user.parts.len(), 3);
        assert!(
            matches!(&user.parts[1], AgentTranscriptPart::Artifact { artifact }
            if artifact.kind == AgentArtifactKind::ResourceLink
                && artifact.label() == "Source"
                && artifact.size == Some(128))
        );
        assert!(
            matches!(&user.parts[2], AgentTranscriptPart::Artifact { artifact }
            if artifact.kind == AgentArtifactKind::TextResource
                && artifact.text.as_deref() == Some("# Context"))
        );

        let agent = &transcript.messages[1];
        assert!(
            matches!(&agent.parts[0], AgentTranscriptPart::Artifact { artifact }
            if artifact.kind == AgentArtifactKind::Image
                && artifact.encoded_data.as_deref() == Some("aW1hZ2U="))
        );
        let AgentTranscriptPart::Artifact { artifact: image } = &agent.parts[0] else {
            unreachable!()
        };
        assert_eq!(image.decoded_data().unwrap().unwrap(), b"image");
        assert_eq!(
            image.bounded_image_data_url(1_024).unwrap().as_deref(),
            Some("data:image/png;base64,aW1hZ2U=")
        );
        assert!(
            matches!(&agent.parts[1], AgentTranscriptPart::Artifact { artifact }
            if artifact.kind == AgentArtifactKind::Audio)
        );
        assert!(matches!(&agent.parts[2], AgentTranscriptPart::ToolUse {
            status: AgentToolStatus::Completed,
            detail: AgentToolDetail::Fetch { output: Some(output) },
            artifacts,
            ..
        } if output == "Fetched"
            && artifacts.len() == 1
            && artifacts[0].kind == AgentArtifactKind::BlobResource
            && artifacts[0].encoded_data.as_deref() == Some("YmluYXJ5")));
        assert_eq!(agent.stop, Some(AgentStopReason::EndTurn));
    }

    #[test]
    fn scheduled_action_contract_preserves_scheduler_snake_case() {
        let request = CreateScheduledAction {
            enabled: true,
            kind: ScheduledActionKind::Agent,
            name: "Daily triage".into(),
            schedule: "0 9 * * 1-5".into(),
            task: serde_json::to_value(AgentScheduledTask {
                model: "claude-sonnet-4-5".into(),
                prompt: "Triage the inbox".into(),
                user_prompt: "Run the daily triage".into(),
            })
            .unwrap(),
            timezone: "America/New_York".into(),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["kind"], "Agent");
        assert_eq!(value["task"]["user_prompt"], "Run the daily triage");
        assert_eq!(value["timezone"], "America/New_York");

        let record: ActionExecutionRecord = serde_json::from_value(serde_json::json!({
            "action_id":"action-1",
            "created_at":"2026-08-27T12:00:00Z",
            "end_time":"2026-08-27T12:01:00Z",
            "is_success":true,
            "resource_id":"session-1",
            "result":{"summary":"done"},
            "start_time":"2026-08-27T12:00:01Z"
        }))
        .unwrap();
        assert_eq!(record.resource_id.as_deref(), Some("session-1"));
    }

    #[test]
    fn crm_contract_matches_storage_openapi_casing() {
        assert_eq!(
            serde_json::to_value(CreateCrmCompanyRequest {
                domain: "acme.com".into(),
                name: "Acme".into(),
            })
            .unwrap(),
            serde_json::json!({"domain":"acme.com","name":"Acme"})
        );
        assert_eq!(
            serde_json::to_value(SetCrmEmailSyncRequest { email_sync: true }).unwrap(),
            serde_json::json!({"email_sync":true})
        );

        let company: CrmCompanyResponse = serde_json::from_value(serde_json::json!({
            "contacts":[{
                "companyId":"company-1",
                "createdAt":"2026-08-27T12:00:00Z",
                "email":"jane@acme.com",
                "firstInteraction":"2026-08-01T12:00:00Z",
                "hidden":false,
                "id":"contact-1",
                "lastInteraction":"2026-08-27T12:00:00Z",
                "name":"Jane",
                "updatedAt":"2026-08-27T12:00:00Z"
            }],
            "createdAt":"2026-08-01T12:00:00Z",
            "description":"Widgets",
            "domains":[{
                "companyId":"company-1",
                "createdAt":"2026-08-01T12:00:00Z",
                "domain":"acme.com",
                "id":"domain-1"
            }],
            "emailSync":true,
            "hidden":false,
            "id":"company-1",
            "name":"Acme",
            "teamId":"team-1",
            "updatedAt":"2026-08-27T12:00:00Z"
        }))
        .unwrap();
        assert!(company.email_sync);
        assert_eq!(company.contacts[0].email, "jane@acme.com");

        let thread: CrmCommentThread = serde_json::from_value(serde_json::json!({
            "thread": {
                "threadId": "018f00b6-f99b-7f22-85ac-5a81ea1aa010",
                "entityType": "crm_company",
                "entityId": "018f00b6-f99b-7f22-85ac-5a81ea1aa011",
                "owner": "macro|owner@example.com",
                "resolved": false,
                "metadata": {"source":"desktop"},
                "createdAt": "2026-08-27T12:00:00Z",
                "updatedAt": "2026-08-27T12:00:01Z",
                "deletedAt": null
            },
            "comments": [{
                "commentId": "018f00b6-f99b-7f22-85ac-5a81ea1aa012",
                "threadId": "018f00b6-f99b-7f22-85ac-5a81ea1aa010",
                "order": 0,
                "owner": "macro|owner@example.com",
                "sender": null,
                "text": "Follow up with @Alex",
                "metadata": {"mentions":{"mentionId":"mention-1","users":["macro|alex@example.com"]}},
                "createdAt": "2026-08-27T12:00:00Z",
                "updatedAt": "2026-08-27T12:00:00Z",
                "deletedAt": null
            }]
        }))
        .unwrap();
        assert_eq!(thread.thread.entity_type, CrmCommentEntityType::CrmCompany);
        assert_eq!(thread.comments[0].order, Some(0));
        assert_eq!(
            CrmCommentEntityType::CrmContact.as_path_segment(),
            "crm_contact"
        );
        assert_eq!(
            serde_json::to_value(CreateCrmCommentRequest {
                text: "A reply".into(),
                thread_id: Some(thread.thread.thread_id.clone()),
                thread_metadata: None,
                metadata: Some(serde_json::json!({
                    "mentions":{"mentionId":"mention-2","users":["macro|alex@example.com"]}
                })),
            })
            .unwrap(),
            serde_json::json!({
                "text":"A reply",
                "threadId":"018f00b6-f99b-7f22-85ac-5a81ea1aa010",
                "metadata":{"mentions":{"mentionId":"mention-2","users":["macro|alex@example.com"]}}
            })
        );

        let settings = UpdateCrmTeamSettingsRequest {
            delete_records_role: Some(CrmPermissionRole::Owner),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(settings).unwrap(),
            serde_json::json!({"delete_records_role":"owner"})
        );
        assert_eq!(
            serde_json::to_value(UpdateCrmTeamSettingsRequest {
                closed_stage_ids: Some(None),
                default_team_view_id: Some(None),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({"closed_stage_ids":null,"default_team_view_id":null})
        );

        let views: SavedViewsResponse = serde_json::from_value(serde_json::json!({
            "views":[{
                "id":"018f00b6-f99b-7f22-85ac-5a81ea1aa001",
                "userId":"user-1",
                "name":"Priority accounts",
                "config":{"kind":"crm","viewMode":"board","isDefault":true},
                "createdAt":"2026-08-27T12:00:00Z",
                "updatedAt":"2026-08-27T12:00:00Z"
            }],
            "excludedDefaultViews":[{
                "id":"018f00b6-f99b-7f22-85ac-5a81ea1aa002",
                "userId":"user-1",
                "defaultViewId":"recent"
            }]
        }))
        .unwrap();
        assert_eq!(views.views[0].name, "Priority accounts");
        assert_eq!(views.excluded_default_views[0].default_view_id, "recent");
        assert_eq!(
            serde_json::to_value(ExcludeDefaultViewRequest {
                default_view_id: "recent".into()
            })
            .unwrap(),
            serde_json::json!({"defaultViewId":"recent"})
        );
        assert_eq!(
            serde_json::to_value(PatchSavedViewRequest {
                name: None,
                config: Some(serde_json::json!({"isDefault":false})),
            })
            .unwrap(),
            serde_json::json!({"config":{"isDefault":false}})
        );
    }

    #[test]
    fn channel_message_contract_matches_generated_storage_shape() {
        let request = ChannelMessageRequest {
            attachments: Vec::new(),
            content: "Hello".into(),
            mentions: vec![SimpleMention {
                entity_id: "user-1".into(),
                entity_type: "USER".into(),
            }],
            nonce: Some("optimistic-1".into()),
            thread_id: Some("parent-1".into()),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["content"], "Hello");
        assert_eq!(value["thread_id"], "parent-1");
        assert_eq!(value["mentions"][0]["entity_id"], "user-1");
        assert_eq!(value["attachments"], serde_json::json!([]));

        let patch = serde_json::to_value(PatchChannelMessageRequest {
            attachment_ids_to_delete: Some(vec!["attachment-1".into()]),
            attachments_to_add: Some(vec![NewChannelAttachment {
                entity_id: "image-2".into(),
                entity_type: "static/image".into(),
                height: Some(900),
                width: Some(1440),
            }]),
            content: Some("Updated".into()),
            mentions: Some(vec![SimpleMention {
                entity_id: "bot-1".into(),
                entity_type: "bot".into(),
            }]),
            nonce: Some("patch-1".into()),
        })
        .unwrap();
        assert_eq!(
            patch,
            serde_json::json!({
                "attachment_ids_to_delete":["attachment-1"],
                "attachments_to_add":[{
                    "entity_id":"image-2","entity_type":"static/image",
                    "height":900,"width":1440
                }],
                "content":"Updated",
                "mentions":[{"entity_id":"bot-1","entity_type":"bot"}],
                "nonce":"patch-1"
            })
        );
    }

    #[test]
    fn channel_activity_matches_generated_unread_boundary_contract() {
        assert_eq!(
            serde_json::to_value(RecordChannelActivityRequest {
                channel_id: "channel-1".into(),
                activity_type: ChannelActivityType::View,
            })
            .unwrap(),
            serde_json::json!({"channel_id":"channel-1","activity_type":"view"})
        );
        let activity: ChannelActivity = serde_json::from_value(serde_json::json!({
            "id":"activity-1","user_id":"user-1","channel_id":"channel-1",
            "created_at":"2026-08-28T12:00:00Z","updated_at":"2026-08-28T12:01:00Z",
            "viewed_at":"2026-08-28T12:01:00Z","interacted_at":null
        }))
        .unwrap();
        assert_eq!(activity.viewed_at.as_deref(), Some("2026-08-28T12:01:00Z"));
    }

    #[test]
    fn channel_message_windows_encode_older_newer_and_center_cursors() {
        assert_eq!(
            channel_messages_path(
                "channel/1",
                100,
                Some("older+cursor"),
                Some("newer/cursor"),
                Some("message 1"),
            ),
            "/channels/channel%2F1/messages?limit=100&cursor=older%2Bcursor&previous_cursor=newer%2Fcursor&load_around_message_id=message%201"
        );

        let resolved: ResolvedChannelMessage = serde_json::from_value(serde_json::json!({
            "channel_id":"channel-1",
            "created_at":"2026-08-28T12:00:00Z",
            "kind":"threadReply",
            "message_id":"reply-1",
            "thread_id":"root-1"
        }))
        .unwrap();
        assert_eq!(resolved.kind, ChannelMessageKind::ThreadReply);
        assert_eq!(resolved.thread_id, "root-1");
    }

    #[test]
    fn attachment_backlinks_match_generated_storage_union() {
        let response: GetAttachmentReferencesResponse = serde_json::from_value(serde_json::json!({
            "references":[
                {
                    "reference_type":"channel",
                    "attachment_created_at":"2026-08-27T12:01:00Z",
                    "channel_id":"channel-1",
                    "channel_name":"design",
                    "message_content":"See the attached brief",
                    "message_created_at":"2026-08-27T12:00:00Z",
                    "message_id":"message-1",
                    "sender_id":"user-1",
                    "thread_id":null
                },
                {
                    "reference_type":"generic",
                    "created_at":"2026-08-27T12:02:00Z",
                    "entity_id":"document-1",
                    "entity_type":"document",
                    "source_entity_id":"chat-1",
                    "source_entity_type":"chat",
                    "user_id":"user-1"
                }
            ]
        }))
        .unwrap();
        assert!(matches!(
            &response.references[0],
            AttachmentEntityReference::Channel { channel_id, .. } if channel_id == "channel-1"
        ));
        assert!(matches!(
            &response.references[1],
            AttachmentEntityReference::Generic { source_entity_id, .. }
                if source_entity_id == "chat-1"
        ));
    }

    #[test]
    fn channel_reaction_and_typing_enums_follow_generated_storage_casing() {
        let add = serde_json::to_value(ChannelReactionRequest {
            action: ReactionAction::Add,
            emoji: "👍".into(),
            message_id: "message-1".into(),
            nonce: None,
        })
        .unwrap();
        assert_eq!(add["action"], "Add");

        let typing = serde_json::to_value(ChannelTypingRequest {
            action: TypingAction::Start,
            nonce: None,
            thread_id: Some("thread-1".into()),
        })
        .unwrap();
        assert_eq!(typing["action"], "start");
        assert_eq!(typing["thread_id"], "thread-1");
    }

    #[test]
    fn channel_create_and_patch_follow_generated_contract() {
        let create = serde_json::to_value(CreateChannelRequest {
            auto_join_team: false,
            channel_type: ChannelType::DirectMessage,
            name: None,
            participants: vec!["user-2".into()],
            team_id: None,
        })
        .unwrap();
        assert_eq!(create["channel_type"], "direct_message");
        assert_eq!(create["participants"][0], "user-2");

        let patch = serde_json::to_value(PatchChannelRequest {
            auto_join_team: Some(true),
            channel_name: Some("engineering".into()),
            convert_to_team_channel: None,
        })
        .unwrap();
        assert_eq!(patch["channel_name"], "engineering");
        assert_eq!(patch["auto_join_team"], true);
        assert!(patch.get("convert_to_team_channel").is_none());
    }

    #[test]
    fn channel_conversation_and_join_link_contracts_match_generated_storage_sdk() {
        assert_eq!(
            serde_json::to_value(GetOrCreateDmRequest {
                recipient_id: "user-2".into(),
            })
            .unwrap(),
            serde_json::json!({"recipient_id":"user-2"})
        );
        assert_eq!(
            serde_json::to_value(GetOrCreatePrivateRequest {
                recipients: vec!["user-2".into(), "user-3".into()],
            })
            .unwrap(),
            serde_json::json!({"recipients":["user-2", "user-3"]})
        );
        let existing: GetOrCreateChannelResponse = serde_json::from_value(serde_json::json!({
            "action":"get",
            "channel_id":"channel-1"
        }))
        .unwrap();
        assert_eq!(existing.action, GetOrCreateChannelAction::Get);
        assert_eq!(existing.channel_id, "channel-1");
        assert_eq!(
            serde_json::from_value::<ChannelJoinCodeResponse>(serde_json::json!({
                "join_code":"code/with+characters"
            }))
            .unwrap()
            .join_code,
            "code/with+characters"
        );
    }

    #[test]
    fn reminder_schedule_and_patch_follow_camel_case_contract() {
        let create = serde_json::to_value(CreateReminderRequest {
            description: "Follow up".into(),
            entity_id: Some("doc-1".into()),
            entity_type: Some("document".into()),
            schedule: ReminderSchedule::Once {
                remind_at: "2026-08-28T13:00:00Z".into(),
            },
        })
        .unwrap();
        assert_eq!(create["entityId"], "doc-1");
        assert_eq!(create["schedule"]["type"], "once");
        assert_eq!(create["schedule"]["remindAt"], "2026-08-28T13:00:00Z");

        let patch = serde_json::to_value(UpdateReminderRequest {
            completed: Some(true),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(patch, serde_json::json!({"completed": true}));
    }

    #[test]
    fn notification_bulk_request_uses_notification_ids_wire_key() {
        let value = serde_json::to_value(NotificationBulkRequest {
            notification_ids: vec!["notification-1".into()],
        })
        .unwrap();
        assert_eq!(
            value,
            serde_json::json!({"notificationIds": ["notification-1"]})
        );
        assert_eq!(
            serde_json::to_value(NotificationEntityInput {
                entity_id: "document-1".into(),
                entity_type: "DOCUMENT".into(),
            })
            .unwrap(),
            serde_json::json!({"entityId":"document-1","entityType":"DOCUMENT"})
        );
        assert_eq!(
            serde_json::to_value(NotificationUpdateOperation::MarkDone).unwrap(),
            serde_json::json!("MARK_DONE")
        );
        let preferences: NotificationTypePreferences = serde_json::from_value(serde_json::json!({
            "disabled_types":["new_email","channel_mention"]
        }))
        .unwrap();
        assert_eq!(preferences.disabled_types[0], "new_email");
        let muted: Vec<NotificationUnsubscribe> = serde_json::from_value(serde_json::json!([{
            "item_id":"channel-1",
            "item_type":"channel"
        }]))
        .unwrap();
        assert_eq!(muted[0].item_type, "channel");
    }

    #[test]
    fn account_and_team_contracts_match_generated_auth_sdk() {
        assert_eq!(
            serde_json::to_value(GetProfilePicturesRequestBody {
                user_id_list: vec!["user-1".into(), "user-2".into()],
            })
            .unwrap(),
            serde_json::json!({"user_id_list":["user-1", "user-2"]})
        );
        let pictures: ProfilePictures = serde_json::from_value(serde_json::json!({
            "pictures":[{
                "checksum":null,
                "id":"user-1",
                "url":"https://static.example.test/file/avatar-1"
            }]
        }))
        .unwrap();
        assert_eq!(pictures.pictures[0].id, "user-1");
        assert_eq!(pictures.pictures[0].checksum, None);
        assert_eq!(
            serde_json::to_value(PatchUserOnboardingRequest {
                first_name: "Nick".into(),
                last_name: "Gardner".into(),
                title: "Founder".into(),
                industry: "Software".into(),
            })
            .unwrap(),
            serde_json::json!({
                "firstName":"Nick",
                "lastName":"Gardner",
                "title":"Founder",
                "industry":"Software"
            })
        );
        assert_eq!(
            serde_json::to_value(CreateCheckoutSessionRequest {
                cancel_url: "macro://settings".into(),
                discount: None,
                success_url: "macro://settings?upgraded=true".into(),
            })
            .unwrap(),
            serde_json::json!({
                "cancelUrl":"macro://settings",
                "successUrl":"macro://settings?upgraded=true"
            })
        );
        assert_eq!(
            serde_json::to_value(PatchTeamRequest {
                default_link_share: Some(Some("TEAM".into())),
                name: Some("Acme".into()),
                slug: Some("ACME".into()),
                user_role_updates: Some(vec![PatchTeamUserRole {
                    role: TeamRole::Admin,
                    team_user_id: "user-2".into(),
                }]),
            })
            .unwrap(),
            serde_json::json!({
                "default_link_share":"TEAM",
                "name":"Acme",
                "slug":"ACME",
                "user_role_updates":[{"role":"admin","team_user_id":"user-2"}]
            })
        );
        assert_eq!(
            serde_json::to_value(PatchTeamRequest {
                default_link_share: Some(None),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({"default_link_share":null})
        );

        let team: TeamWithMembers = serde_json::from_value(serde_json::json!({
            "team": {
                "allow_non_admin_invites":true,
                "auto_join_domain":null,
                "crm_enabled":true,
                "default_link_share":"TEAM",
                "enterprise":false,
                "id":"team-1",
                "name":"Acme",
                "owner_id":"user-1",
                "slug":"ACME"
            },
            "members":[{"role":"owner","team_id":"team-1","user_id":"user-1"}]
        }))
        .unwrap();
        assert_eq!(team.members[0].role, TeamRole::Owner);
        assert_eq!(team.team.default_link_share.as_deref(), Some("TEAM"));

        let starter_documents: StarterDocumentsResponse =
            serde_json::from_value(serde_json::json!({"how_to_guide_id":"document-how-to"}))
                .unwrap();
        assert_eq!(starter_documents.how_to_guide_id, "document-how-to");
    }

    #[test]
    fn integration_contracts_match_cognition_openapi() {
        assert_eq!(
            serde_json::to_value(AddMcpServerRequest {
                server_name: "Linear".into(),
                url: "https://mcp.example.com".into(),
            })
            .unwrap(),
            serde_json::json!({
                "server_name":"Linear",
                "url":"https://mcp.example.com"
            })
        );
        let catalog: PipedreamCatalogResponse = serde_json::from_value(serde_json::json!({
            "next_cursor":"next",
            "servers":[{
                "app_slug":"linear",
                "description":"Issue tracking",
                "display_name":"Linear",
                "icon_url":"https://example.com/linear.png"
            }]
        }))
        .unwrap();
        assert_eq!(catalog.servers[0].app_slug, "linear");
        assert_eq!(catalog.next_cursor.as_deref(), Some("next"));
    }

    #[test]
    fn bot_and_webhook_contracts_match_generated_storage_sdk() {
        assert_eq!(
            serde_json::to_value(CreateChannelScopedBotRequest {
                avatar_url: None,
                description: Some("Answers support questions".into()),
                handle: "support".into(),
                has_agent: Some(true),
                name: "Support bot".into(),
                team_id: None,
                token_expires_at: None,
                token_label: Some("GPUI desktop".into()),
            })
            .unwrap(),
            serde_json::json!({
                "description":"Answers support questions",
                "handle":"support",
                "has_agent":true,
                "name":"Support bot",
                "token_label":"GPUI desktop"
            })
        );

        let request = CreateWebhookRequest {
            endpoint_url: "https://hooks.example.com/macro".into(),
            filters: vec![WebhookFilter {
                events: vec!["document.created".into(), "document.updated".into()],
                ids: None,
            }],
            headers: Some(BTreeMap::from([("x-tenant".into(), "acme".into())])),
            name: "Document mirror".into(),
            namespace: "document-mirror".into(),
            scope: WebhookScope::Team,
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "endpoint_url":"https://hooks.example.com/macro",
                "filters":[{"events":["document.created","document.updated"]}],
                "headers":{"x-tenant":"acme"},
                "name":"Document mirror",
                "namespace":"document-mirror",
                "scope":"team"
            })
        );

        let created: CreateWebhookResponse = serde_json::from_value(serde_json::json!({
            "created_at":"2026-08-28T12:00:00Z",
            "created_by_user_id":"user-1",
            "deleted_at":null,
            "endpoint_url":"https://hooks.example.com/macro",
            "filters":[{"events":["document.created"],"ids":["doc-1"]}],
            "headers":{},
            "id":"webhook-1",
            "is_valid":false,
            "name":"Document mirror",
            "namespace":"document-mirror",
            "signing_secret":"whsec_once",
            "status":"active",
            "updated_at":"2026-08-28T12:00:00Z",
            "workspace_id":"team-1"
        }))
        .unwrap();
        assert_eq!(created.webhook.status, WebhookStatus::Active);
        assert_eq!(created.signing_secret, "whsec_once");
    }

    #[test]
    fn github_and_cursor_contracts_match_generated_auth_sdk() {
        let github: InitGithubLinkResponse = serde_json::from_value(serde_json::json!({
            "authorization_url":"https://github.com/login/oauth/authorize",
            "link_id":"018f0000-0000-7000-8000-000000000001"
        }))
        .unwrap();
        assert!(github.authorization_url.starts_with("https://github.com/"));

        let branch: BranchNameResponse = serde_json::from_value(serde_json::json!({
            "shortId":"8YxQ4m",
            "branchName":"task/8YxQ4m-ship-native-task-parity"
        }))
        .unwrap();
        assert_eq!(branch.short_id, "8YxQ4m");
        assert_eq!(branch.branch_name, "task/8YxQ4m-ship-native-task-parity");

        let status: CursorApiKeyStatus = serde_json::from_value(serde_json::json!({
            "registered":true,
            "defaultModelId":"grok-4.6",
            "updatedAt":"2026-08-27T12:00:00Z"
        }))
        .unwrap();
        assert!(status.registered);
        assert_eq!(status.default_model_id.as_deref(), Some("grok-4.6"));
        assert_eq!(
            serde_json::to_value(PutCursorApiKeyRequest {
                api_key: "crsr_secret".into(),
            })
            .unwrap(),
            serde_json::json!({"apiKey":"crsr_secret"})
        );
        assert_eq!(
            serde_json::to_value(PutCursorDefaultModelRequest {
                model_id: Some("grok-4.6".into()),
            })
            .unwrap(),
            serde_json::json!({"modelId":"grok-4.6"})
        );
        let models: CursorModelsResponse = serde_json::from_value(serde_json::json!({
            "models":[{"displayName":"Cursor Grok 4.6","id":"grok-4.6"}]
        }))
        .unwrap();
        assert_eq!(models.models[0].display_name, "Cursor Grok 4.6");
    }

    #[test]
    fn task_github_pull_requests_parse_and_merge_live_enrichment() {
        let stored: GithubPullRequestsResponse = serde_json::from_value(serde_json::json!({
            "pullRequests":[{
                "displayName":"macro#42",
                "foreignEntityId":"foreign-42",
                "githubKey":"macro-inc/macro/pull/42",
                "number":42,
                "owner":"macro-inc",
                "repo":"macro",
                "url":"https://github.com/macro-inc/macro/pull/42"
            }]
        }))
        .unwrap();
        let enriched: GithubPullRequestsResponse = serde_json::from_value(serde_json::json!({
            "pullRequests":[{
                "additions":120,
                "authorLogin":"octocat",
                "checks":[{
                    "conclusion":"success",
                    "id":7,
                    "name":"CI",
                    "status":"completed",
                    "url":"https://github.com/macro-inc/macro/actions/runs/7"
                }],
                "comments":[],
                "deletions":14,
                "description":"Implements native task PR parity",
                "displayName":"macro#42",
                "githubKey":"macro-inc/macro/pull/42",
                "name":"feat: native task PRs",
                "number":42,
                "owner":"macro-inc",
                "repo":"macro",
                "status":"open",
                "url":"https://github.com/macro-inc/macro/pull/42"
            }]
        }))
        .unwrap();
        let merged = merge_github_pull_requests(stored.pull_requests, enriched.pull_requests);
        assert_eq!(merged[0].foreign_entity_id.as_deref(), Some("foreign-42"));
        assert_eq!(merged[0].name.as_deref(), Some("feat: native task PRs"));
        assert_eq!(merged[0].status.as_deref(), Some("open"));
        assert_eq!(
            merged[0].checks.as_ref().unwrap()[0].conclusion.as_deref(),
            Some("success")
        );

        let request = EnrichGithubPullRequestsRequest {
            pull_requests: vec![GithubPullRequestRef {
                display_name: merged[0].display_name.clone(),
                github_key: merged[0].github_key.clone(),
                number: merged[0].number,
                owner: merged[0].owner.clone(),
                repo: merged[0].repo.clone(),
                url: merged[0].url.clone(),
            }],
        };
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({"pullRequests":[{
                "displayName":"macro#42",
                "githubKey":"macro-inc/macro/pull/42",
                "number":42,
                "owner":"macro-inc",
                "repo":"macro",
                "url":"https://github.com/macro-inc/macro/pull/42"
            }]})
        );
    }

    #[test]
    fn project_contracts_preserve_storage_camel_case_and_root_moves() {
        assert_eq!(
            serde_json::to_value(CreateProjectRequest {
                name: "Design".into(),
                project_parent_id: Some("root-1".into()),
            })
            .unwrap(),
            serde_json::json!({"name":"Design","projectParentId":"root-1"})
        );
        assert_eq!(
            serde_json::to_value(PatchProjectRequest {
                project_parent_id: Some(None),
                ..Default::default()
            })
            .unwrap(),
            serde_json::json!({"projectParentId":null})
        );
        let response: ProjectsResponse = serde_json::from_value(serde_json::json!({
            "data":[{
                "createdAt":"2026-08-27T12:00:00Z",
                "deletedAt":null,
                "id":"project-1",
                "name":"Design",
                "parentId":null,
                "updatedAt":"2026-08-27T12:00:00Z",
                "userId":"user-1"
            }],
            "error":false
        }))
        .unwrap();
        assert_eq!(response.data[0].name, "Design");
        assert!(response.data[0].parent_id.is_none());
    }

    #[test]
    fn email_filter_contracts_match_generated_email_sdk() {
        assert_eq!(
            serde_json::to_value(UpsertEmailFilterRequest {
                email_address: Some("sender@example.com".into()),
                email_domain: None,
                is_important: false,
            })
            .unwrap(),
            serde_json::json!({
                "email_address":"sender@example.com",
                "is_important":false
            })
        );

        let response: UpsertEmailFilterResponse = serde_json::from_value(serde_json::json!({
            "filter":{
                "created_at":"2026-08-28T12:00:00Z",
                "email_address":"sender@example.com",
                "email_domain":null,
                "id":"018f0000-0000-7000-8000-000000000001",
                "is_important":true
            }
        }))
        .unwrap();
        assert!(response.filter.is_important);
        assert_eq!(
            response.filter.email_address.as_deref(),
            Some("sender@example.com")
        );
    }

    #[test]
    fn grouped_soup_contract_matches_generated_storage_sdk() {
        let group_by = SoupGroupByField::Property {
            property_definition_id: "00000001-0000-0000-0000-000000000002".into(),
            entity_type: Some(SoupPropertyEntityType::Task),
        };
        assert_eq!(
            serde_json::to_value(group_by).unwrap(),
            serde_json::json!({
                "property": {
                    "property_definition_id":"00000001-0000-0000-0000-000000000002",
                    "entity_type":"TASK"
                }
            })
        );
        assert_eq!(
            serde_json::to_value(GroupedSoupSort::ViewedUpdated).unwrap(),
            serde_json::json!("viewed_updated")
        );

        let page: GroupedSoupPage = serde_json::from_value(serde_json::json!({
            "mode":"initial",
            "items":{"task-1":{"tag":"document","data":{"id":"task-1"}}},
            "groups":[{
                "key":"00000001-0000-0000-0000-000000000002",
                "label":"In Progress",
                "display_order":2,
                "total_count":12,
                "item_ids":["task-1"],
                "next_cursor":"opaque"
            }]
        }))
        .unwrap();
        let GroupedSoupPage::Initial { items, groups } = page else {
            panic!("expected initial grouped page");
        };
        assert!(items.contains_key("task-1"));
        assert_eq!(groups[0].total_count, 12);
        assert_eq!(groups[0].next_cursor.as_deref(), Some("opaque"));

        let filtered = grouped_soup_filter_object(&serde_json::json!({
            "limit":100,
            "expand":true,
            "sort_method":"created_at",
            "sort_direction":"asc",
            "df":{"l":{"dst":"task"}}
        }))
        .unwrap();
        assert_eq!(filtered.len(), 1);
        assert!(filtered.contains_key("df"));
    }

    #[test]
    fn calendar_contracts_match_generated_email_sdk() {
        let request = CreateCalendarEventRequest {
            attendees: vec![CalendarAttendeeInput {
                email: "person@example.com".into(),
                is_optional: Some(true),
            }],
            calendar_id: Some("calendar-1".into()),
            conference: Some("google_meet".into()),
            description: Some("Agenda".into()),
            email_link_id: None,
            location: Some("Studio".into()),
            recurrence_lines: vec!["RRULE:FREQ=WEEKLY".into()],
            time: CalendarEventTime::Timed {
                starts_at: "2026-08-28T13:00:00Z".into(),
                ends_at: "2026-08-28T14:00:00Z".into(),
                time_zone: Some("America/New_York".into()),
            },
            title: "Planning".into(),
            transparency: Some("opaque".into()),
            visibility: Some("default".into()),
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["calendarId"], "calendar-1");
        assert_eq!(value["attendees"][0]["isOptional"], true);
        assert_eq!(value["time"]["kind"], "timed");
        assert_eq!(value["time"]["startsAt"], "2026-08-28T13:00:00Z");
        assert_eq!(value["conference"], "google_meet");

        let rsvp = serde_json::to_value(RsvpCalendarEventRequest {
            recurrence_id: None,
            response: AttendeeResponseStatus::Accepted,
            scope: Some("all".into()),
        })
        .unwrap();
        assert_eq!(
            rsvp,
            serde_json::json!({"response":"accepted","scope":"all"})
        );

        let event: CalendarEvent = serde_json::from_value(serde_json::json!({
            "attendees":[],
            "calendarId":"calendar-1",
            "conferenceProvider":"google_meet",
            "conferenceUrl":"https://meet.google.com/abc-defg-hij",
            "createdAt":"2026-08-27T12:00:00Z",
            "description":"Agenda",
            "icalUid":"event@example.com",
            "id":"event-1",
            "isReadOnly":false,
            "location":"Studio",
            "organizerEmail":"owner@example.com",
            "ownerId":"user-1",
            "recurrenceLines":[],
            "sequence":1,
            "status":"confirmed",
            "time":{"kind":"allDay","startDate":"2026-08-28","endDate":"2026-08-29"},
            "title":"Planning",
            "transparency":"opaque",
            "updatedAt":"2026-08-27T12:00:00Z",
            "visibility":"default"
        }))
        .unwrap();
        assert!(matches!(event.time, CalendarEventTime::AllDay { .. }));
    }

    #[test]
    fn task_duplicate_contracts_match_storage_api() {
        let response: TaskDuplicatesResponse = serde_json::from_value(serde_json::json!({
            "duplicates":[{
                "id":"match-1",
                "taskId":"task-2",
                "taskName":"Ship release",
                "vectorScore":0.94
            }]
        }))
        .unwrap();
        assert_eq!(response.duplicates[0].task_id, "task-2");
        assert_eq!(response.duplicates[0].task_name, "Ship release");
        assert_eq!(response.duplicates[0].vector_score, 0.94);

        assert_eq!(
            serde_json::to_value(DismissTaskDuplicatesRequest {
                match_ids: vec!["match-1".into(), "match-2".into()],
            })
            .unwrap(),
            serde_json::json!({"matchIds":["match-1","match-2"]})
        );
    }

    #[test]
    fn import_contracts_match_cognition_openapi_and_preserve_metadata() {
        let state: ImportState = serde_json::from_value(serde_json::json!({
            "runs":[{
                "source":"linear",
                "status":"ready",
                "auto_import":false,
                "error":null,
                "updated_at":"2026-08-28T12:00:00Z"
            }],
            "entities":[{
                "created_at":"2026-08-28T11:00:00Z",
                "entity_id":null,
                "entity_type":null,
                "foreign_id":"ENG-142",
                "id":"import-1",
                "initiator":"onboarding",
                "last_error":null,
                "metadata":{
                    "identifier":"ENG-142",
                    "title":"Ship native import UI",
                    "due_date":"2026-08-30",
                    "future_field":{"lossless":true}
                },
                "source":"linear",
                "status":"staged",
                "team_id":"team-1",
                "updated_at":"2026-08-28T12:00:00Z",
                "user_id":"user-1"
            }]
        }))
        .unwrap();
        assert_eq!(state.runs[0].source, ImportSource::Linear);
        assert_eq!(state.entities[0].label(), "Ship native import UI");
        assert_eq!(state.entities[0].metadata["future_field"]["lossless"], true);
        assert!(!state.anything_in_flight());

        assert_eq!(
            serde_json::to_value(RunImportRequest {
                import_ids: vec!["import-1".into()],
                discard_ids: Vec::new(),
            })
            .unwrap(),
            serde_json::json!({"import_ids":["import-1"]})
        );
        let outcome: RunImportOutcome = serde_json::from_value(serde_json::json!({
            "discarded":0,
            "importing":1
        }))
        .unwrap();
        assert_eq!(outcome.importing, 1);
    }

    #[test]
    fn onboarding_contracts_match_cognition_openapi() {
        let state: OnboardingState = serde_json::from_value(serde_json::json!({
            "row": {
                "status": "active",
                "skipped": false,
                "started_at": "2026-08-31T12:00:00Z",
                "completed_at": null
            },
            "connected_servers": [{
                "name": "Notion",
                "url": "https://mcp.example.test/notion",
                "authenticated": true
            }],
            "suggested_team_domain": "example.test"
        }))
        .unwrap();

        assert_eq!(state.row.status, OnboardingStatus::Active);
        assert!(!state.row.skipped);
        assert_eq!(state.connected_servers[0].name, "Notion");
        assert!(state.connected_servers[0].authenticated);
        assert_eq!(state.suggested_team_domain.as_deref(), Some("example.test"));
        assert_eq!(
            serde_json::to_value(CompleteOnboardingRequest { skipped: true }).unwrap(),
            serde_json::json!({"skipped": true})
        );
        assert_eq!(
            serde_json::to_value(PatchUserTutorialRequest {
                tutorial_complete: true,
            })
            .unwrap(),
            serde_json::json!({"tutorialComplete": true})
        );
        assert_eq!(
            account_deletion_feedback_payload(
                "phc_test",
                "macro|alex@example.test",
                "missing_features",
                Some("Needed native parity"),
            ),
            serde_json::json!({
                "api_key":"phc_test",
                "event":"account_deletion_feedback",
                "properties":{
                    "$lib":"macro-gpui",
                    "distinct_id":"macro|alex@example.test",
                    "reason":"missing_features",
                    "feedback":"Needed native parity"
                }
            })
        );
    }
}
