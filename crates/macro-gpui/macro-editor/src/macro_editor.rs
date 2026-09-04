use editor::{
    Editor, EditorEvent, HighlightKey, MultiBufferOffset, NavigationOverlayKey, SelectionEffects,
    scroll::Autoscroll,
};
use gpui::prelude::*;
use gpui::{
    AccessibleAction, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    ListState, Render, Role, Subscription, Window, accesskit::ActionData, list, px,
};
use language::language_settings::SoftWrap;
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use pulldown_cmark::{CodeBlockKind, Event, MetadataBlockKind, Options, Parser, Tag, TagEnd, html};
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use similar::{DiffTag, TextDiff};
use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use text::Point;
use ui::prelude::ActiveTheme;
use ui::{Button, ButtonCommon, ButtonSize, ButtonStyle, Clickable, VisibleOnHover};
use url::Url;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentEditorEvent {
    Changed,
    SelectionChanged,
    SnippetTypeahead(SnippetTypeaheadCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetTypeaheadCommand {
    Previous,
    Next,
    Accept,
    Dismiss,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentMergeConflict {
    pub local: String,
    pub remote: String,
    pub conflict_markers: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentConflictResolution {
    KeepLocal,
    UseRemote,
    ReviewMarkers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommentAnchorError {
    EmptySelection,
    SelectionDoesNotResolveToText,
    NestedComment,
}

impl fmt::Display for CommentAnchorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySelection => {
                write!(formatter, "select text before creating an anchored comment")
            }
            Self::SelectionDoesNotResolveToText => write!(
                formatter,
                "the selected Markdown does not resolve to editable Lexical text"
            ),
            Self::NestedComment => {
                write!(
                    formatter,
                    "comments cannot be nested inside another comment anchor"
                )
            }
        }
    }
}

impl std::error::Error for CommentAnchorError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentStats {
    pub words: usize,
    pub characters: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentAnalysis {
    pub stats: DocumentStats,
    pub outline: Vec<OutlineItem>,
    pub blocks: Vec<DocumentBlockSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub level: u8,
    pub title: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentBlockSummary {
    /// The corresponding Macro Lexical node ID when one is available. Newly
    /// typed, not-yet-projected blocks receive a deterministic native fallback
    /// until the next collaboration projection assigns their durable ID.
    pub stable_id: String,
    pub index: usize,
    pub line: u32,
    pub range: Range<usize>,
    pub kind: DocumentBlockKind,
    pub preview: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentBlockKind {
    Paragraph,
    Heading,
    BulletedList,
    NumberedList,
    Checklist,
    Quote,
    Code,
    Table,
    Divider,
    Media,
}

impl DocumentBlockKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Paragraph => "Text",
            Self::Heading => "Heading",
            Self::BulletedList => "Bullets",
            Self::NumberedList => "Numbered",
            Self::Checklist => "Checklist",
            Self::Quote => "Quote",
            Self::Code => "Code",
            Self::Table => "Table",
            Self::Divider => "Divider",
            Self::Media => "Media",
        }
    }
}

/// Native formatting and block insertion commands shared by documents, tasks,
/// snippets, comments, mail, and channel composers. The visual surface remains
/// Zed's editor; these operations produce Markdown that Macro's Lexical/Loro
/// schema projector can round-trip without introducing a second document model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkdownCommand {
    Paragraph,
    Heading(u8),
    BulletList,
    NumberedList,
    Checklist,
    Quote,
    CodeBlock,
    Divider,
    Table,
    Bold,
    Italic,
    Strikethrough,
    InlineCode,
    Link,
    Image,
    Video,
    InlineMath,
    BlockMath,
    Superscript,
    Subscript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentStructureCommand {
    Indent,
    Outdent,
    MoveBlockUp,
    MoveBlockDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentSlashCommand {
    pub keyword: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub command: MarkdownCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSlashContext {
    pub range: Range<usize>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentSnippetContext {
    pub range: Range<usize>,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocumentSnippetInsertion {
    pub state: Value,
    pub markdown: String,
    pub caret: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroInlineMention {
    User {
        user_id: String,
        email: String,
        display_name: String,
    },
    Document {
        document_id: String,
        document_name: String,
        block_name: String,
    },
    Contact {
        contact_id: String,
        name: String,
        email_or_domain: String,
        is_company: bool,
    },
    Group {
        group_alias: String,
    },
    Date {
        date: String,
        display_format: String,
    },
    Tag {
        option_id: String,
        property_definition_id: String,
        scope: String,
        name: String,
        color: Option<String>,
    },
}

impl MacroInlineMention {
    pub fn visible_token(&self) -> String {
        match self {
            Self::User { display_name, .. }
            | Self::Document {
                document_name: display_name,
                ..
            }
            | Self::Contact {
                name: display_name, ..
            }
            | Self::Group {
                group_alias: display_name,
            }
            | Self::Date {
                display_format: display_name,
                ..
            } => format!("@{display_name}"),
            Self::Tag { name, .. } => format!("#{name}"),
        }
    }

    fn serialized_node(&self) -> Value {
        let metadata = json!({"id": node_id()});
        match self {
            Self::User {
                user_id,
                email,
                display_name,
            } => json!({
                "$": metadata,
                "type": "user-mention",
                "version": 1,
                "userId": user_id,
                "email": email,
                "displayName": display_name,
            }),
            Self::Document {
                document_id,
                document_name,
                block_name,
            } => json!({
                "$": metadata,
                "type": "document-mention",
                "version": 2,
                "documentId": document_id,
                "documentName": document_name,
                "blockName": block_name,
                "blockParams": {},
                "collapsed": false,
            }),
            Self::Contact {
                contact_id,
                name,
                email_or_domain,
                is_company,
            } => json!({
                "$": metadata,
                "type": "contact-mention",
                "version": 1,
                "contactId": contact_id,
                "name": name,
                "emailOrDomain": email_or_domain,
                "isCompany": is_company,
            }),
            Self::Group { group_alias } => json!({
                "$": metadata,
                "type": "group-mention",
                "version": 1,
                "groupAlias": group_alias,
            }),
            Self::Date {
                date,
                display_format,
            } => json!({
                "$": metadata,
                "type": "date-mention",
                "version": 1,
                "date": date,
                "displayFormat": display_format,
            }),
            Self::Tag {
                option_id,
                property_definition_id,
                scope,
                name,
                color,
            } => json!({
                "$": metadata,
                "type": "tag-mention",
                "version": 1,
                "optionId": option_id,
                "propertyDefinitionId": property_definition_id,
                "scope": scope,
                "name": name,
                "color": color,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionInsertionError {
    MissingLabel,
    SelectionNotCollapsed,
    SelectionDoesNotResolveToText,
    ProtectedSchema(LexicalProjectionError),
}

impl fmt::Display for MentionInsertionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLabel => write!(formatter, "Mention labels cannot be empty or multiline"),
            Self::SelectionNotCollapsed => {
                write!(formatter, "Place the caret before inserting a mention")
            }
            Self::SelectionDoesNotResolveToText => write!(
                formatter,
                "The caret is not inside an editable Macro text block"
            ),
            Self::ProtectedSchema(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for MentionInsertionError {}

const DOCUMENT_SLASH_COMMANDS: &[DocumentSlashCommand] = &[
    DocumentSlashCommand {
        keyword: "text paragraph",
        label: "Text",
        description: "Plain paragraph",
        command: MarkdownCommand::Paragraph,
    },
    DocumentSlashCommand {
        keyword: "heading h1 title",
        label: "Heading 1",
        description: "Large section heading",
        command: MarkdownCommand::Heading(1),
    },
    DocumentSlashCommand {
        keyword: "heading h2 subtitle",
        label: "Heading 2",
        description: "Medium section heading",
        command: MarkdownCommand::Heading(2),
    },
    DocumentSlashCommand {
        keyword: "heading h3",
        label: "Heading 3",
        description: "Small section heading",
        command: MarkdownCommand::Heading(3),
    },
    DocumentSlashCommand {
        keyword: "bullet unordered list",
        label: "Bullet list",
        description: "Unordered list",
        command: MarkdownCommand::BulletList,
    },
    DocumentSlashCommand {
        keyword: "number ordered list",
        label: "Numbered list",
        description: "Ordered list",
        command: MarkdownCommand::NumberedList,
    },
    DocumentSlashCommand {
        keyword: "check task todo list",
        label: "Checklist",
        description: "Interactive task list",
        command: MarkdownCommand::Checklist,
    },
    DocumentSlashCommand {
        keyword: "quote citation",
        label: "Quote",
        description: "Quoted text",
        command: MarkdownCommand::Quote,
    },
    DocumentSlashCommand {
        keyword: "code block",
        label: "Code block",
        description: "Fenced source code",
        command: MarkdownCommand::CodeBlock,
    },
    DocumentSlashCommand {
        keyword: "table grid",
        label: "Table",
        description: "Two-column table",
        command: MarkdownCommand::Table,
    },
    DocumentSlashCommand {
        keyword: "divider separator rule",
        label: "Divider",
        description: "Horizontal divider",
        command: MarkdownCommand::Divider,
    },
    DocumentSlashCommand {
        keyword: "image photo media",
        label: "Image",
        description: "Linked image",
        command: MarkdownCommand::Image,
    },
    DocumentSlashCommand {
        keyword: "video movie media",
        label: "Video",
        description: "Linked video",
        command: MarkdownCommand::Video,
    },
    DocumentSlashCommand {
        keyword: "math equation display",
        label: "Equation",
        description: "Display math block",
        command: MarkdownCommand::BlockMath,
    },
];

pub fn document_slash_commands(query: &str) -> Vec<DocumentSlashCommand> {
    let terms = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    DOCUMENT_SLASH_COMMANDS
        .iter()
        .copied()
        .filter(|item| {
            let haystack = format!(
                "{} {} {}",
                item.keyword,
                item.label.to_ascii_lowercase(),
                item.description.to_ascii_lowercase()
            );
            terms.iter().all(|term| haystack.contains(term))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownCommandEdit {
    pub range: Range<usize>,
    pub replacement: String,
    /// Selection in the complete document after applying `replacement`.
    pub selection: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarkdownMultiCommandEdit {
    edits: Vec<MarkdownCommandEdit>,
    text: String,
    selections: Vec<Range<usize>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DocumentSearchOptions {
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentReplaceResult {
    pub text: String,
    pub replacements: usize,
}

/// Versioned clipboard metadata shared by every Macro surface that embeds the
/// native document editor. The ordinary clipboard string remains Markdown so
/// other applications receive useful text, while native Macro paste can retain
/// both the source Markdown and its HTML representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentClipboardPayload {
    pub schema: String,
    pub markdown: String,
    pub html: String,
    pub plain_text: String,
}

impl DocumentClipboardPayload {
    pub const SCHEMA: &'static str = "macro.document-clipboard.v1";

    pub fn from_markdown(markdown: &str) -> Self {
        Self {
            schema: Self::SCHEMA.into(),
            markdown: markdown.into(),
            html: markdown_to_html_fragment(markdown),
            plain_text: markdown_to_plain_text(markdown),
        }
    }

    pub fn from_metadata(metadata: &str) -> Option<Self> {
        let payload = serde_json::from_str::<Self>(metadata).ok()?;
        (payload.schema == Self::SCHEMA).then_some(payload)
    }
}

pub fn markdown_to_html_fragment(markdown: &str) -> String {
    // Macro document clipboard HTML is data, not an executable document.
    // Escape inline/raw HTML rather than letting pulldown-cmark pass it
    // through verbatim to consumers such as email composers.
    let parser = Parser::new_ext(markdown, Options::all()).map(|event| match event {
        Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
        event => event,
    });
    let mut output = String::new();
    html::push_html(&mut output, parser);
    output
}

pub fn markdown_to_plain_text(markdown: &str) -> String {
    let mut output = String::new();
    for event in Parser::new_ext(markdown, Options::all()) {
        match event {
            Event::Text(text) | Event::Code(text) | Event::Html(text) | Event::InlineHtml(text) => {
                output.push_str(&text)
            }
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Rule => output.push_str("\n---\n"),
            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableRow
                | TagEnd::CodeBlock,
            ) => output.push('\n'),
            _ => {}
        }
    }
    let mut normalized = String::with_capacity(output.len());
    let mut consecutive_newlines = 0;
    for character in output.trim().chars() {
        if character == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                normalized.push(character);
            }
        } else {
            consecutive_newlines = 0;
            normalized.push(character);
        }
    }
    normalized
}

/// A cursor position in Macro's stable Lexical node coordinate space. Loro
/// awareness carries this node ID alongside a relative cursor so selections
/// can survive concurrent edits and block movement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalTextPoint {
    pub node_id: String,
    pub unicode_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDocumentSelection {
    pub peer_id: String,
    pub user_id: Option<String>,
    pub color_index: u32,
    pub anchor: usize,
    pub focus: usize,
}

struct MacroRemoteSelectionHighlights;

#[derive(Debug, Clone, PartialEq, Eq)]
struct LexicalTextSpan {
    node_id: String,
    markdown_start: usize,
    markdown_end: usize,
}

/// Three-way merge used when a CRDT projection arrives while the native Zed
/// editor has unsaved text. A conflict is returned with explicit markers and
/// must be resolved before the application is allowed to queue another CRDT
/// update.
pub fn merge_document_text(base: &str, local: &str, remote: &str) -> Result<String, String> {
    diffy::merge(base, local, remote)
}

fn compile_document_search(query: &str, options: DocumentSearchOptions) -> Result<Regex, String> {
    if query.is_empty() {
        return Err("Enter text to find".into());
    }
    let escaped;
    let pattern = if options.regex {
        query
    } else {
        escaped = regex::escape(query);
        &escaped
    };
    RegexBuilder::new(pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|error| format!("Invalid regular expression: {error}"))
}

fn search_word_character(character: Option<char>) -> bool {
    character.is_some_and(|character| character == '_' || character.is_alphanumeric())
}

fn is_whole_word_match(text: &str, range: &Range<usize>) -> bool {
    !search_word_character(text[..range.start].chars().next_back())
        && !search_word_character(text[range.end..].chars().next())
}

/// Returns byte ranges so matches can be applied directly to Zed's
/// `MultiBufferOffset` coordinate space. Rust's regex engine guarantees UTF-8
/// boundaries, including for case-insensitive Unicode searches.
pub fn document_search_matches(
    text: &str,
    query: &str,
    options: DocumentSearchOptions,
) -> Result<Vec<Range<usize>>, String> {
    let expression = compile_document_search(query, options)?;
    Ok(expression
        .find_iter(text)
        .map(|found| found.start()..found.end())
        .filter(|range| !options.whole_word || is_whole_word_match(text, range))
        .collect())
}

/// Applies every match without losing regex capture expansion. Plain-text
/// replacement deliberately treats `$` literally, matching native editor
/// expectations rather than accidentally interpreting it as a capture.
pub fn replace_all_document_matches(
    text: &str,
    query: &str,
    replacement: &str,
    options: DocumentSearchOptions,
) -> Result<DocumentReplaceResult, String> {
    let expression = compile_document_search(query, options)?;
    let mut output = String::with_capacity(text.len());
    let mut previous_end = 0;
    let mut replacements = 0;
    for captures in expression.captures_iter(text) {
        let Some(found) = captures.get(0) else {
            continue;
        };
        let range = found.start()..found.end();
        if options.whole_word && !is_whole_word_match(text, &range) {
            continue;
        }
        output.push_str(&text[previous_end..range.start]);
        if options.regex {
            captures.expand(replacement, &mut output);
        } else {
            output.push_str(replacement);
        }
        previous_end = range.end;
        replacements += 1;
    }
    if replacements == 0 {
        return Ok(DocumentReplaceResult {
            text: text.to_owned(),
            replacements: 0,
        });
    }
    output.push_str(&text[previous_end..]);
    Ok(DocumentReplaceResult {
        text: output,
        replacements,
    })
}

/// Detects a slash query only when it is the first non-whitespace content on
/// the active line. This prevents URLs, prose slashes, and inline code from
/// unexpectedly opening the block menu.
pub fn document_slash_context(text: &str, cursor: usize) -> Option<DocumentSlashContext> {
    let cursor = floor_char_boundary(text, cursor);
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let prefix = &text[line_start..cursor];
    let indentation = prefix.len() - prefix.trim_start().len();
    let slash_start = line_start + indentation;
    let query = text[slash_start..cursor].strip_prefix('/')?;
    if !query.chars().all(|character| {
        character.is_alphanumeric() || character.is_whitespace() || matches!(character, '-' | '_')
    }) {
        return None;
    }
    Some(DocumentSlashContext {
        range: slash_start..cursor,
        query: query.to_owned(),
    })
}

/// Detect Macro's snippet typeahead trigger. The web client only creates the
/// `;` search node at a text-node boundary or after whitespace and refuses to
/// activate inside code. The native projection does not expose Lexical text
/// nodes, so this applies the equivalent rule to the Markdown buffer.
pub fn document_snippet_context(text: &str, cursor: usize) -> Option<DocumentSnippetContext> {
    let cursor = floor_char_boundary(text, cursor.min(text.len()));
    if text[cursor..]
        .chars()
        .next()
        .is_some_and(|character| !character.is_whitespace())
    {
        return None;
    }
    let prefix = &text[..cursor];
    let code_ranges = markdown_code_ranges(text);
    for (start, character) in prefix.char_indices().rev() {
        if character == '\n' || character == '\r' {
            break;
        }
        if character != ';' {
            continue;
        }
        let before = &text[..start];
        if before
            .chars()
            .next_back()
            .is_some_and(|character| !character.is_whitespace())
        {
            continue;
        }
        let query = &text[start + 1..cursor];
        // A second semicolon is Macro's escape hatch for a literal trigger.
        // Leading whitespace closes a newly opened menu in the web client.
        if query.contains(';')
            || query.chars().next().is_some_and(char::is_whitespace)
            || code_ranges
                .iter()
                .any(|range| range.start <= start && start < range.end)
        {
            return None;
        }
        return Some(DocumentSnippetContext {
            range: start..cursor,
            query: query.trim().to_owned(),
        });
    }
    None
}

/// Inserts a raw Macro snippet tree into an existing native projection.
///
/// A unique marker is first projected through the current lossless Lexical
/// source. Replacing that marker in the tree preserves unknown nodes and the
/// exact identity payload of existing mentions. The three insertion cases
/// mirror Macro's `$replaceAwaitNodeWithSnippet`: replace an otherwise-empty
/// paragraph, unwrap a single paragraph inline, or insert multiple blocks
/// after the active top-level block.
pub fn insert_snippet_lexical_state(
    source: Option<&Value>,
    current_markdown: &str,
    trigger_range: Range<usize>,
    snippet_state: &Value,
) -> Result<DocumentSnippetInsertion, String> {
    if trigger_range.start > trigger_range.end || trigger_range.end > current_markdown.len() {
        return Err("snippet trigger range is outside the document".into());
    }
    if !current_markdown.is_char_boundary(trigger_range.start)
        || !current_markdown.is_char_boundary(trigger_range.end)
    {
        return Err("snippet trigger range is not on a character boundary".into());
    }

    let marker = format!("\u{e000}macro-snippet-{}\u{e001}", Uuid::new_v4());
    let mut marked_markdown = current_markdown.to_owned();
    marked_markdown.replace_range(trigger_range, &marker);
    let mut state = match source {
        Some(source) => project_markdown_edits(source, &marked_markdown)
            .map_err(|kind| format!("cannot insert beside protected {kind} content"))?,
        None => markdown_to_lexical_state(&marked_markdown),
    };
    let marked_projection = lexical_state_to_markdown(&state);
    let marker_offset = marked_projection
        .find(&marker)
        .ok_or_else(|| "snippet insertion marker was not preserved".to_owned())?;
    split_snippet_marker_node(&mut state, &marker)?;

    let snippet_root = snippet_state.get("root").unwrap_or(snippet_state);
    let snippet_children = snippet_root
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "snippet raw state does not contain root children".to_owned())?;
    let root = if state.get("root").is_some() {
        state
            .get_mut("root")
            .expect("root presence was checked immediately above")
    } else {
        &mut state
    };
    let root_children = root
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "document state does not contain root children".to_owned())?;
    let top_index = root_children
        .iter()
        .position(|node| lexical_node_contains_text(node, &marker))
        .ok_or_else(|| "snippet insertion marker could not be located".to_owned())?;

    let caret = if snippet_children.is_empty() {
        remove_lexical_text_node(&mut root_children[top_index], &marker);
        marker_offset
    } else {
        let marker_is_only_paragraph_child =
            root_children[top_index].get("type").and_then(Value::as_str) == Some("paragraph")
                && root_children[top_index]
                    .get("children")
                    .and_then(Value::as_array)
                    .is_some_and(|children| {
                        children.len() == 1 && lexical_node_is_exact_text(&children[0], &marker)
                    });
        let single_paragraph = (snippet_children.len() == 1
            && snippet_children[0].get("type").and_then(Value::as_str) == Some("paragraph"))
        .then(|| {
            snippet_children[0]
                .get("children")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        });

        if marker_is_only_paragraph_child {
            let inserted_count = snippet_children.len();
            root_children.splice(top_index..=top_index, snippet_children);
            rendered_root_prefix_end(root_children, top_index + inserted_count - 1)
        } else if let Some(inline_children) = single_paragraph {
            let inline_markdown = inline_children
                .iter()
                .map(|child| render_node(child, RenderContext::Inline))
                .collect::<String>();
            if !replace_lexical_text_node(&mut root_children[top_index], &marker, inline_children) {
                return Err("snippet insertion marker could not be replaced".into());
            }
            marker_offset + inline_markdown.len()
        } else {
            remove_lexical_text_node(&mut root_children[top_index], &marker);
            let inserted_count = snippet_children.len();
            root_children.splice(top_index + 1..top_index + 1, snippet_children);
            rendered_root_prefix_end(root_children, top_index + inserted_count)
        }
    };

    let markdown = lexical_state_to_markdown(&state);
    Ok(DocumentSnippetInsertion {
        caret: floor_char_boundary(&markdown, caret.min(markdown.len())),
        markdown,
        state,
    })
}

fn split_snippet_marker_node(node: &mut Value, marker: &str) -> Result<(), String> {
    let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) else {
        if let Some(root) = node.get_mut("root") {
            return split_snippet_marker_node(root, marker);
        }
        return Ok(());
    };
    for index in 0..children.len() {
        if is_textual_lexical_node(
            children[index]
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ) && let Some(text) = children[index].get("text").and_then(Value::as_str)
            && let Some(marker_start) = text.find(marker)
        {
            if text[marker_start + marker.len()..].contains(marker) {
                return Err("snippet insertion marker appeared more than once".into());
            }
            let original = children[index].clone();
            let before = &text[..marker_start];
            let after = &text[marker_start + marker.len()..];
            let mut replacement = Vec::with_capacity(3);
            if !before.is_empty() {
                let mut node = original.clone();
                set_text_node_content(&mut node, before, Some(node_id()));
                replacement.push(node);
            }
            let mut marker_node = original.clone();
            set_text_node_content(&mut marker_node, marker, Some(node_id()));
            replacement.push(marker_node);
            if !after.is_empty() {
                let mut node = original;
                set_text_node_content(&mut node, after, Some(node_id()));
                replacement.push(node);
            }
            children.splice(index..=index, replacement);
            return Ok(());
        }
        if lexical_node_contains_text(&children[index], marker) {
            return split_snippet_marker_node(&mut children[index], marker);
        }
    }
    Ok(())
}

fn lexical_node_contains_text(node: &Value, needle: &str) -> bool {
    node.get("text")
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains(needle))
        || node
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children
                    .iter()
                    .any(|child| lexical_node_contains_text(child, needle))
            })
}

fn lexical_node_is_exact_text(node: &Value, text: &str) -> bool {
    is_textual_lexical_node(node.get("type").and_then(Value::as_str).unwrap_or_default())
        && node.get("text").and_then(Value::as_str) == Some(text)
}

fn replace_lexical_text_node(node: &mut Value, text: &str, replacement: Vec<Value>) -> bool {
    let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) else {
        return false;
    };
    for index in 0..children.len() {
        if lexical_node_is_exact_text(&children[index], text) {
            children.splice(index..=index, replacement);
            return true;
        }
        if lexical_node_contains_text(&children[index], text)
            && replace_lexical_text_node(&mut children[index], text, replacement.clone())
        {
            return true;
        }
    }
    false
}

fn remove_lexical_text_node(node: &mut Value, text: &str) -> bool {
    replace_lexical_text_node(node, text, Vec::new())
}

fn rendered_root_prefix_end(children: &[Value], inclusive_index: usize) -> usize {
    let mut length = 0;
    let mut rendered_any = false;
    for child in children.iter().take(inclusive_index.saturating_add(1)) {
        let rendered = render_node(child, RenderContext::Block);
        if rendered.is_empty() {
            continue;
        }
        if rendered_any {
            length += 2;
        }
        rendered_any = true;
        length += rendered.len();
    }
    length
}

fn replacement_for_document_match(
    text: &str,
    query: &str,
    replacement: &str,
    options: DocumentSearchOptions,
    target: &Range<usize>,
) -> Result<String, String> {
    let expression = compile_document_search(query, options)?;
    for captures in expression.captures_iter(text) {
        let Some(found) = captures.get(0) else {
            continue;
        };
        if found.start() == target.start && found.end() == target.end {
            let mut output = String::new();
            if options.regex {
                captures.expand(replacement, &mut output);
            } else {
                output.push_str(replacement);
            }
            return Ok(output);
        }
    }
    Err("The selected match changed before it could be replaced".into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineChange {
    Equal(String),
    Added(String),
    Removed(String),
}

/// A native Markdown edit cannot safely rewrite a block containing a Lexical
/// node whose schema is not implemented by this client. Keeping this error
/// typed lets the application explain the exact protected block without ever
/// replacing the authoritative CRDT value with a lossy projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalProjectionError {
    pub block_index: usize,
    pub node_type: String,
}

impl fmt::Display for LexicalProjectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "block {} contains an unsupported Macro Lexical node ({}) and was edited; its data was left unchanged",
            self.block_index + 1,
            self.node_type
        )
    }
}

impl std::error::Error for LexicalProjectionError {}

/// Produces a stable line-oriented diff for history previews. The exact LCS
/// path is used for ordinary documents; very large comparisons deliberately
/// fall back to a bounded remove/add representation so opening history can
/// never monopolize the GPUI thread.
pub fn line_diff(before: &str, after: &str) -> Vec<LineChange> {
    let before = before.lines().collect::<Vec<_>>();
    let after = after.lines().collect::<Vec<_>>();
    if before.len().saturating_mul(after.len()) > 4_000_000 {
        return before
            .into_iter()
            .map(|line| LineChange::Removed(line.to_owned()))
            .chain(
                after
                    .into_iter()
                    .map(|line| LineChange::Added(line.to_owned())),
            )
            .collect();
    }

    let mut lengths = vec![vec![0_u32; after.len() + 1]; before.len() + 1];
    for left in (0..before.len()).rev() {
        for right in (0..after.len()).rev() {
            lengths[left][right] = if before[left] == after[right] {
                lengths[left + 1][right + 1] + 1
            } else {
                lengths[left + 1][right].max(lengths[left][right + 1])
            };
        }
    }

    let (mut left, mut right) = (0, 0);
    let mut changes = Vec::new();
    while left < before.len() && right < after.len() {
        if before[left] == after[right] {
            changes.push(LineChange::Equal(before[left].to_owned()));
            left += 1;
            right += 1;
        } else if lengths[left + 1][right] >= lengths[left][right + 1] {
            changes.push(LineChange::Removed(before[left].to_owned()));
            left += 1;
        } else {
            changes.push(LineChange::Added(after[right].to_owned()));
            right += 1;
        }
    }
    changes.extend(
        before[left..]
            .iter()
            .map(|line| LineChange::Removed((*line).to_owned())),
    );
    changes.extend(
        after[right..]
            .iter()
            .map(|line| LineChange::Added((*line).to_owned())),
    );
    changes
}

/// A document-scoped Zed editor surface. The application owns transport and
/// collaboration; this type owns native text input, IME, selection, clipboard,
/// find/replace, and undo/redo behavior through Zed's editor implementation.
pub struct NativeDocumentEditor {
    document_id: String,
    name: String,
    editor: Entity<Editor>,
    preview_focus_handle: FocusHandle,
    preview_enabled: bool,
    rich_block_cursor: usize,
    rich_block_list: ListState,
    next_native_block_id: u64,
    rich_inline_editor: Entity<Editor>,
    rich_inline_block_id: Option<String>,
    applying_inline_change: bool,
    analysis: DocumentAnalysis,
    rich_blocks: Vec<RichDocumentBlock>,
    lexical_source: Option<Value>,
    last_synced_text: String,
    remote_conflict: Option<DocumentMergeConflict>,
    remote_selections: Vec<RemoteDocumentSelection>,
    dirty: bool,
    read_only: bool,
    applying_remote: bool,
    dismissed_snippet_context: Option<DocumentSnippetContext>,
    _subscription: Subscription,
    _inline_subscription: Subscription,
}

struct RichDocumentBlock {
    stable_id: String,
    index: usize,
    markdown: String,
    preview: Entity<Markdown>,
}

#[derive(Clone)]
struct DraggedRichDocumentBlock {
    document_id: String,
    stable_id: String,
    source_index: usize,
    label: String,
}

struct RichDocumentBlockDragPreview {
    label: String,
}

impl Render for RichDocumentBlockDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        gpui::div()
            .max_w(px(420.0))
            .px_3()
            .py_2()
            .rounded_md()
            .border_1()
            .border_color(colors.border_focused)
            .bg(colors.elevated_surface_background)
            .text_color(colors.text)
            .shadow_md()
            .child(self.label.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RichPreviewCommand {
    Previous,
    Next,
    First,
    Last,
    Edit,
}

fn rich_preview_command(key: &str, modified: bool) -> Option<RichPreviewCommand> {
    if modified {
        return None;
    }
    match key {
        "up" | "k" => Some(RichPreviewCommand::Previous),
        "down" | "j" => Some(RichPreviewCommand::Next),
        "home" => Some(RichPreviewCommand::First),
        "end" => Some(RichPreviewCommand::Last),
        "enter" | "e" => Some(RichPreviewCommand::Edit),
        _ => None,
    }
}

fn snippet_typeahead_command(key: &str, modified: bool) -> Option<SnippetTypeaheadCommand> {
    if modified {
        return None;
    }
    match key {
        "up" => Some(SnippetTypeaheadCommand::Previous),
        "down" => Some(SnippetTypeaheadCommand::Next),
        "enter" => Some(SnippetTypeaheadCommand::Accept),
        "escape" => Some(SnippetTypeaheadCommand::Dismiss),
        _ => None,
    }
}

fn safe_markdown_external_url(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    matches!(parsed.scheme(), "http" | "https")
        .then_some(())
        .filter(|_| parsed.host_str().is_some())?;
    Some(parsed.to_string())
}

impl NativeDocumentEditor {
    pub fn new(
        document_id: String,
        name: String,
        text: String,
        read_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_synced_text = text.clone();
        let analysis = document_analysis(&text);
        let rich_blocks = Self::create_rich_blocks(&text, &analysis, cx);
        let next_native_block_id = rich_blocks.len() as u64;
        let rich_block_list =
            ListState::new(rich_blocks.len(), gpui::ListAlignment::Top, px(480.0))
                .with_uniform_item_height(px(72.0));
        let preview_focus_handle = cx.focus_handle().tab_stop(true);
        let editor = cx.new(|cx| {
            let mut editor = Editor::multi_line(window, cx);
            editor.set_text(text, window, cx);
            editor.set_read_only(read_only);
            editor.set_placeholder_text("Start writing…", window, cx);
            editor.set_soft_wrap_mode(SoftWrap::EditorWidth, cx);
            editor.set_show_gutter(false, cx);
            editor.set_show_line_numbers(false, cx);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            editor
        });
        let rich_inline_editor = cx.new(|cx| {
            // Grow ordinary blocks with their wrapped content while bounding
            // pathological/code-heavy blocks to a contained scrolling editor.
            let mut editor = Editor::auto_height(2, 12, window, cx);
            editor.set_read_only(read_only);
            editor.set_soft_wrap_mode(SoftWrap::EditorWidth, cx);
            editor.set_show_gutter(false, cx);
            editor.set_show_line_numbers(false, cx);
            editor.set_show_wrap_guides(false, cx);
            editor.set_show_indent_guides(false, cx);
            editor
        });
        let subscription = cx.subscribe_in(&editor, window, |this, _, event, _, cx| match event {
            EditorEvent::BufferEdited => {
                let text = this.editor.read(cx).text(cx);
                this.refresh_document_views(&text, cx);
                if !this.applying_remote {
                    let was_dirty = this.dirty;
                    this.dirty = text != this.last_synced_text;
                    // Editor initialization may publish a deferred buffer event
                    // after the subscription is installed. Do not turn an
                    // unchanged freshly opened document into an unsaved edit.
                    // A real edit, including a return to the saved value, still
                    // emits so the application's autosync generation can settle.
                    if this.dirty || was_dirty {
                        cx.emit(DocumentEditorEvent::Changed);
                    }
                }
                cx.notify();
            }
            EditorEvent::SelectionsChanged { local: true } if !this.applying_remote => {
                cx.emit(DocumentEditorEvent::SelectionChanged);
            }
            _ => {}
        });
        let inline_subscription = cx.subscribe_in(
            &rich_inline_editor,
            window,
            |this, inline_editor, event, window, cx| {
                if matches!(event, EditorEvent::BufferEdited) && !this.applying_inline_change {
                    let text = inline_editor.read(cx).text(cx);
                    this.apply_rich_inline_text(text, window, cx);
                }
            },
        );
        Self {
            document_id,
            name,
            editor,
            preview_focus_handle,
            preview_enabled: true,
            rich_block_cursor: 0,
            rich_block_list,
            next_native_block_id,
            rich_inline_editor,
            rich_inline_block_id: None,
            applying_inline_change: false,
            analysis,
            rich_blocks,
            lexical_source: None,
            last_synced_text,
            remote_conflict: None,
            remote_selections: Vec::new(),
            dirty: false,
            read_only,
            applying_remote: false,
            dismissed_snippet_context: None,
            _subscription: subscription,
            _inline_subscription: inline_subscription,
        }
    }

    fn create_rich_blocks(
        text: &str,
        analysis: &DocumentAnalysis,
        cx: &mut Context<Self>,
    ) -> Vec<RichDocumentBlock> {
        analysis
            .blocks
            .iter()
            .filter_map(|block| {
                let markdown = text.get(block.range.clone())?.to_owned();
                let preview_markdown = markdown.clone();
                Some(RichDocumentBlock {
                    stable_id: block.stable_id.clone(),
                    index: block.index,
                    markdown,
                    preview: cx.new(|cx| Markdown::new(preview_markdown.into(), None, None, cx)),
                })
            })
            .collect()
    }

    fn stabilize_native_block_ids(&mut self, analysis: &mut DocumentAnalysis, text: &str) {
        let previous = &self.rich_blocks;
        if previous.is_empty() || analysis.blocks.is_empty() {
            for block in &mut analysis.blocks {
                if is_positional_native_block_id(&block.stable_id) {
                    block.stable_id = self.fresh_native_block_id();
                }
            }
            return;
        }

        let next_markdown = analysis
            .blocks
            .iter()
            .map(|block| text.get(block.range.clone()).unwrap_or_default().to_owned())
            .collect::<Vec<_>>();
        let mut resolved = analysis
            .blocks
            .iter()
            .map(|block| {
                (!is_positional_native_block_id(&block.stable_id)).then(|| block.stable_id.clone())
            })
            .collect::<Vec<_>>();
        let mut previous_used = vec![false; previous.len()];

        for id in resolved.iter().flatten() {
            if let Some(index) = previous.iter().position(|block| block.stable_id == *id) {
                previous_used[index] = true;
            }
        }

        let common_prefix = previous
            .iter()
            .zip(&next_markdown)
            .take_while(|(before, after)| before.markdown == **after)
            .count();
        for index in 0..common_prefix {
            if resolved[index].is_none() && !previous_used[index] {
                resolved[index] = Some(previous[index].stable_id.clone());
                previous_used[index] = true;
            }
        }

        let mut previous_suffix = previous.len();
        let mut next_suffix = next_markdown.len();
        while previous_suffix > common_prefix
            && next_suffix > common_prefix
            && previous[previous_suffix - 1].markdown == next_markdown[next_suffix - 1]
        {
            previous_suffix -= 1;
            next_suffix -= 1;
            if resolved[next_suffix].is_none() && !previous_used[previous_suffix] {
                resolved[next_suffix] = Some(previous[previous_suffix].stable_id.clone());
                previous_used[previous_suffix] = true;
            }
        }

        // Preserve exact blocks that moved. A unique unmatched source is safe;
        // duplicate text remains unresolved rather than borrowing the wrong ID.
        for next_index in 0..next_markdown.len() {
            if resolved[next_index].is_some() {
                continue;
            }
            let candidates = previous
                .iter()
                .enumerate()
                .filter(|(index, block)| {
                    !previous_used[*index] && block.markdown == next_markdown[next_index]
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if let [previous_index] = candidates.as_slice() {
                resolved[next_index] = Some(previous[*previous_index].stable_id.clone());
                previous_used[*previous_index] = true;
            }
        }

        // Content changes inside the active inline editor still describe the
        // same block. Its old position is the strongest available identity for
        // Markdown that has not received a durable Lexical node ID yet.
        if let Some(active_id) = self.rich_inline_block_id.as_deref()
            && !resolved.iter().flatten().any(|id| id == active_id)
            && let Some(previous_index) = previous
                .iter()
                .position(|block| block.stable_id == active_id)
            && !previous_used[previous_index]
        {
            let preferred = previous_index.min(resolved.len().saturating_sub(1));
            let target = (preferred..resolved.len())
                .chain((0..preferred).rev())
                .find(|index| resolved[*index].is_none());
            if let Some(target) = target {
                resolved[target] = Some(active_id.to_owned());
                previous_used[previous_index] = true;
            }
        }

        // When structure did not change, unmatched positions represent edited
        // blocks. Preserve their identity even when all text changed.
        if previous.len() == resolved.len() {
            for index in 0..resolved.len() {
                if resolved[index].is_none() && !previous_used[index] {
                    resolved[index] = Some(previous[index].stable_id.clone());
                    previous_used[index] = true;
                }
            }
        }

        for (block, id) in analysis.blocks.iter_mut().zip(resolved) {
            if is_positional_native_block_id(&block.stable_id) {
                block.stable_id = id.unwrap_or_else(|| self.fresh_native_block_id());
            }
        }
    }

    fn fresh_native_block_id(&mut self) -> String {
        let id = format!("native-session-block-{}", self.next_native_block_id);
        self.next_native_block_id = self.next_native_block_id.saturating_add(1);
        id
    }

    fn refresh_document_views(&mut self, text: &str, cx: &mut Context<Self>) {
        let previous_block_ids = self
            .rich_blocks
            .iter()
            .map(|block| block.stable_id.clone())
            .collect::<Vec<_>>();
        let selected_block_id = self
            .rich_blocks
            .get(self.rich_block_cursor)
            .map(|block| block.stable_id.clone());
        let mut analysis = document_analysis_with_source(text, self.lexical_source.as_ref());
        self.stabilize_native_block_ids(&mut analysis, text);
        let mut existing = std::mem::take(&mut self.rich_blocks)
            .into_iter()
            .map(|block| (block.stable_id.clone(), block))
            .collect::<HashMap<_, _>>();
        let mut rich_blocks = Vec::with_capacity(analysis.blocks.len());
        let mut changed_blocks = Vec::new();
        for block in &analysis.blocks {
            let Some(markdown) = text.get(block.range.clone()).map(ToOwned::to_owned) else {
                continue;
            };
            let mut rich_block = if let Some(existing) = existing.remove(&block.stable_id) {
                if existing.markdown != markdown {
                    changed_blocks.push(rich_blocks.len());
                    let updated = markdown.clone();
                    existing
                        .preview
                        .update(cx, |preview, cx| preview.replace(updated, cx));
                }
                existing
            } else {
                let preview_markdown = markdown.clone();
                RichDocumentBlock {
                    stable_id: block.stable_id.clone(),
                    index: block.index,
                    markdown: markdown.clone(),
                    preview: cx.new(|cx| Markdown::new(preview_markdown.into(), None, None, cx)),
                }
            };
            rich_block.index = block.index;
            rich_block.markdown = markdown;
            rich_blocks.push(rich_block);
        }
        self.analysis = analysis;
        self.rich_blocks = rich_blocks;
        self.rich_block_cursor = selected_block_id
            .and_then(|selected_block_id| {
                self.rich_blocks
                    .iter()
                    .position(|block| block.stable_id == selected_block_id)
            })
            .unwrap_or_else(|| {
                self.rich_block_cursor
                    .min(self.rich_blocks.len().saturating_sub(1))
            });

        let current_block_ids = self
            .rich_blocks
            .iter()
            .map(|block| block.stable_id.clone())
            .collect::<Vec<_>>();
        if previous_block_ids == current_block_ids {
            for index in changed_blocks {
                self.rich_block_list.remeasure_items(index..index + 1);
            }
        } else {
            self.rich_block_list
                .reset_with_uniform_height(self.rich_blocks.len(), px(72.0));
            if !self.rich_blocks.is_empty() {
                self.rich_block_list
                    .scroll_to_reveal_item(self.rich_block_cursor);
            }
        }
    }

    fn rich_preview_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.snippet_typeahead_key_down(event, cx) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        if self.rich_inline_block_id.is_some() {
            if event.keystroke.key == "escape"
                && !(modifiers.control || modifiers.alt || modifiers.platform || modifiers.function)
            {
                self.finish_rich_inline_edit(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        let Some(command) = rich_preview_command(
            event.keystroke.key.as_str(),
            modifiers.control || modifiers.alt || modifiers.platform || modifiers.function,
        ) else {
            return;
        };

        match command {
            RichPreviewCommand::Previous => {
                self.rich_block_cursor = self.rich_block_cursor.saturating_sub(1);
            }
            RichPreviewCommand::Next => {
                self.rich_block_cursor =
                    (self.rich_block_cursor + 1).min(self.rich_blocks.len().saturating_sub(1));
            }
            RichPreviewCommand::First => self.rich_block_cursor = 0,
            RichPreviewCommand::Last => {
                self.rich_block_cursor = self.rich_blocks.len().saturating_sub(1);
            }
            RichPreviewCommand::Edit if !self.read_only => {
                self.begin_rich_inline_edit(self.rich_block_cursor, None, window, cx);
            }
            RichPreviewCommand::Edit => return,
        }
        if !self.rich_blocks.is_empty() {
            self.rich_block_list
                .scroll_to_reveal_item(self.rich_block_cursor);
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn begin_rich_inline_edit(
        &mut self,
        position: usize,
        source_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let Some(block) = self.rich_blocks.get(position) else {
            return false;
        };
        let stable_id = block.stable_id.clone();
        let markdown = block.markdown.clone();
        let caret = floor_char_boundary(&markdown, source_index.unwrap_or(0).min(markdown.len()));
        let point = point_for_offset(&markdown, caret);
        self.rich_block_cursor = position;
        self.rich_inline_block_id = Some(stable_id);
        self.applying_inline_change = true;
        self.rich_inline_editor.update(cx, |editor, cx| {
            editor.set_text(markdown, window, cx);
            // The inline editor entity is reused between blocks. Its initial
            // set_text transaction and any history from the previous block
            // must not be reachable from this block's Cmd-Z stack.
            let buffer = editor.buffer().clone();
            while let Some(transaction_id) = buffer.read(cx).last_transaction_id(cx) {
                buffer.update(cx, |buffer, cx| {
                    buffer.forget_transaction(transaction_id, cx)
                });
            }
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([point..point]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        self.applying_inline_change = false;
        self.rich_block_list.scroll_to_reveal_item(position);
        self.rich_block_list.remeasure_items(position..position + 1);
        cx.notify();
        true
    }

    fn finish_rich_inline_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(stable_id) = self.rich_inline_block_id.take() {
            if let Some(position) = self
                .rich_blocks
                .iter()
                .position(|block| block.stable_id == stable_id)
            {
                self.rich_block_list.remeasure_items(position..position + 1);
            }
            self.preview_focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    fn apply_rich_inline_text(
        &mut self,
        replacement: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(stable_id) = self.rich_inline_block_id.as_deref() else {
            return false;
        };
        let Some(block) = self
            .rich_blocks
            .iter()
            .find(|block| block.stable_id == stable_id)
        else {
            self.finish_rich_inline_edit(window, cx);
            return false;
        };
        let block_index = block.index;
        let document_text = self.text(cx);
        // Several IME composition updates can be delivered before the main
        // editor's BufferEdited subscription refreshes cached analysis. Always
        // derive the active range from the authoritative current buffer so a
        // previous multibyte composition update cannot leave a stale byte
        // offset inside the next character.
        let Some(range) = markdown_block_ranges(&document_text)
            .get(block_index)
            .cloned()
        else {
            self.finish_rich_inline_edit(window, cx);
            return false;
        };
        if document_text.get(range.clone()) == Some(replacement.as_str()) {
            return true;
        }
        // Inline mode owns exactly one rendered block. Creating a Markdown
        // block boundary applies the edit atomically, then returns to preview
        // mode so the newly split blocks receive their own stable list rows.
        let created_block_boundary = markdown_block_ranges(&replacement).len() > 1;
        let removed_block = replacement.trim().is_empty();
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(range.start)..MultiBufferOffset(range.end),
                    replacement,
                )],
                cx,
            );
        });
        self.rich_block_list
            .remeasure_items(self.rich_block_cursor..self.rich_block_cursor + 1);
        if created_block_boundary || removed_block {
            self.finish_rich_inline_edit(window, cx);
        } else {
            self.rich_inline_editor
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        }
        cx.notify();
        true
    }

    fn sync_rich_inline_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(stable_id) = self.rich_inline_block_id.as_deref() else {
            return;
        };
        let Some((position, markdown)) = self
            .rich_blocks
            .iter()
            .enumerate()
            .find(|(_, block)| block.stable_id == stable_id)
            .map(|(position, block)| (position, block.markdown.clone()))
        else {
            self.finish_rich_inline_edit(window, cx);
            return;
        };
        self.rich_block_cursor = position;
        if self.rich_inline_editor.read(cx).text(cx) != markdown {
            self.applying_inline_change = true;
            self.rich_inline_editor.update(cx, |editor, cx| {
                editor.set_text(markdown, window, cx);
            });
            self.applying_inline_change = false;
        }
    }

    fn edit_rich_block_at_source_index(
        &mut self,
        block_index: usize,
        source_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(position) = self
            .rich_blocks
            .iter()
            .position(|block| block.index == block_index)
        else {
            return false;
        };
        self.begin_rich_inline_edit(position, Some(source_index), window, cx)
    }

    fn toggle_rich_block_checkbox(
        &mut self,
        block_index: usize,
        source_range: Range<usize>,
        new_checked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let text = self.text(cx);
        let Some(block) = self
            .analysis
            .blocks
            .iter()
            .find(|block| block.index == block_index)
        else {
            return false;
        };
        let block_length = block.range.end.saturating_sub(block.range.start);
        if source_range.start > source_range.end || source_range.end > block_length {
            return false;
        }
        let global_range =
            (block.range.start + source_range.start)..(block.range.start + source_range.end);
        let Some(existing_marker) = text.get(global_range.clone()) else {
            return false;
        };
        if !matches!(existing_marker, "[ ]" | "[x]" | "[X]") {
            return false;
        }
        let replacement = if new_checked { "[x]" } else { "[ ]" };
        if existing_marker == replacement {
            return true;
        }
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(global_range.start)..MultiBufferOffset(global_range.end),
                    replacement,
                )],
                cx,
            );
        });
        self.preview_focus_handle.focus(window, cx);
        cx.notify();
        true
    }

    fn render_rich_block(
        &mut self,
        position: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(block) = self.rich_blocks.get(position) else {
            return gpui::div().into_any_element();
        };
        let stable_id = block.stable_id.clone();
        let index = block.index;
        let preview = block.preview.clone();
        let active = position == self.rich_block_cursor;
        let read_only = self.read_only;
        let inline_editing = self.rich_inline_block_id.as_deref() == Some(stable_id.as_str());
        let inline_editor = self.rich_inline_editor.clone();
        let inline_editor_for_accessibility = inline_editor.downgrade();
        let inline_focus_handle = inline_editor.read(cx).focus_handle(cx);
        let inline_text = inline_editing.then(|| inline_editor.read(cx).text(cx));
        let style = MarkdownStyle::themed(MarkdownFont::Preview, window, cx);
        let colors = cx.theme().colors();
        let selected_background = colors.element_active.opacity(0.4);
        let selected_border = colors.border_focused;
        let transparent_border = selected_border.opacity(0.0);
        let hover_background = colors.element_hover;
        let label = self
            .analysis
            .blocks
            .iter()
            .find(|summary| summary.index == index)
            .map(|summary| format!("{}: {}", summary.kind.label(), summary.preview))
            .unwrap_or_else(|| format!("Document block {}", position + 1));
        let hover_group = format!("native-rich-block-group-{stable_id}");
        let inline_input_id = format!("native-rich-block-input-{stable_id}");
        let inline_input_label = format!("Edit {label}");
        let markdown_view = cx.entity().downgrade();
        let checkbox_view = markdown_view.clone();
        let accessibility_view = markdown_view.clone();
        let row_block_id = stable_id.clone();
        let clicked_block_id = row_block_id.clone();
        let drag_target_id = stable_id.clone();
        let accessible_block_id = stable_id.clone();
        let drop_document_id = self.document_id.clone();
        let dragged_block = DraggedRichDocumentBlock {
            document_id: self.document_id.clone(),
            stable_id: stable_id.clone(),
            source_index: index,
            label: label.clone(),
        };
        let markdown_element = MarkdownElement::new(preview, style)
            .on_url_click(|url, _, cx| {
                if let Some(url) = safe_markdown_external_url(&url) {
                    cx.open_url(&url);
                }
            })
            .on_source_click(move |source_index, click_count, window, cx| {
                let Some(view) = markdown_view.upgrade() else {
                    return false;
                };
                view.update(cx, |this, cx| {
                    this.rich_block_cursor = position;
                    let _ = click_count;
                    this.edit_rich_block_at_source_index(index, source_index, window, cx)
                })
            });
        let markdown_element = if read_only {
            markdown_element
        } else {
            markdown_element.on_checkbox_toggle(move |source_range, new_checked, window, cx| {
                let Some(view) = checkbox_view.upgrade() else {
                    return;
                };
                view.update(cx, |this, cx| {
                    this.rich_block_cursor = position;
                    this.toggle_rich_block_checkbox(index, source_range, new_checked, window, cx);
                });
            })
        };

        let row = gpui::div()
            .id(format!("native-rich-block-{stable_id}"))
            .group(hover_group.clone())
            .role(Role::ListItem)
            .aria_label(label)
            .aria_selected(active)
            .focusable()
            .tab_stop(true)
            .focus_visible(|this| this.bg(hover_background))
            .w_full()
            .max_w(px(840.0))
            .mx_auto()
            .mb_3()
            .flex()
            .items_start()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_l_2()
            .border_color(if active {
                selected_border
            } else {
                transparent_border
            })
            .when(active, |this| this.bg(selected_background))
            .hover(|this| this.bg(hover_background))
            .when(!inline_editing, |this| this.cursor_pointer())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.rich_block_cursor = position;
                this.rich_block_list.scroll_to_reveal_item(position);
                if this.preview_enabled
                    && this.rich_inline_block_id.as_deref() != Some(clicked_block_id.as_str())
                {
                    this.preview_focus_handle.focus(window, cx);
                }
                cx.notify();
            }))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                let Some(view) = accessibility_view.upgrade() else {
                    return;
                };
                view.update(cx, |this, cx| {
                    this.rich_block_cursor = position;
                    this.rich_block_list.scroll_to_reveal_item(position);
                    if this.rich_inline_block_id.as_deref() == Some(accessible_block_id.as_str()) {
                        this.rich_inline_editor
                            .read(cx)
                            .focus_handle(cx)
                            .focus(window, cx);
                    } else {
                        this.preview_focus_handle.focus(window, cx);
                    }
                    cx.notify();
                });
            });
        let row = if read_only || inline_editing {
            row
        } else {
            row.drag_over::<DraggedRichDocumentBlock>(move |this, dragged, _, _| {
                if dragged.document_id == drop_document_id && dragged.stable_id != drag_target_id {
                    this.border_b_2().border_color(selected_border)
                } else {
                    this
                }
            })
            .on_drop(cx.listener(
                move |this, dragged: &DraggedRichDocumentBlock, window, cx| {
                    if dragged.document_id != this.document_id
                        || dragged.source_index == index
                        || dragged.stable_id == row_block_id
                    {
                        return;
                    }
                    if let Some(source_position) = this
                        .rich_blocks
                        .iter()
                        .position(|block| block.stable_id == dragged.stable_id)
                    {
                        this.rich_block_cursor = source_position;
                    }
                    this.move_block_to(dragged.source_index, index, window, cx);
                },
            ))
        };
        row.when(!read_only, |this| {
            this.child(
                gpui::div()
                    .id(format!("drag-native-rich-block-{stable_id}"))
                    .w(px(18.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(colors.text_muted)
                    .cursor(gpui::CursorStyle::OpenHand)
                    .child("☷")
                    .on_drag(dragged_block, |block, _, _, cx| {
                        let label = block.label.clone();
                        cx.new(|_| RichDocumentBlockDragPreview { label })
                    }),
            )
            .child(
                gpui::div()
                    .when(!active && !inline_editing, |this| {
                        this.visible_on_hover(hover_group)
                    })
                    .child(
                        Button::new(
                            format!("edit-native-rich-block-{stable_id}"),
                            if inline_editing { "Done" } else { "Edit" },
                        )
                        .tab_index(0isize)
                        .style(ButtonStyle::Transparent)
                        .size(ButtonSize::Compact)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if inline_editing {
                                    this.finish_rich_inline_edit(window, cx);
                                } else {
                                    this.begin_rich_inline_edit(position, None, window, cx);
                                }
                            },
                        )),
                    ),
            )
        })
        .child(
            gpui::div()
                .id(inline_input_id)
                .min_w_0()
                .flex_1()
                .when(inline_editing, |this| {
                    this.role(Role::TextInput)
                        .aria_label(inline_input_label)
                        .aria_value(inline_text.unwrap_or_default())
                        .track_focus(&inline_focus_handle)
                        .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                            let Some(ActionData::Value(value)) = data else {
                                return;
                            };
                            let Some(editor) = inline_editor_for_accessibility.upgrade() else {
                                return;
                            };
                            editor.update(cx, |editor, cx| {
                                editor.set_text(value.to_string(), window, cx);
                            });
                        })
                        .child(inline_editor)
                })
                .when(!inline_editing, |this| this.child(markdown_element)),
        )
        .into_any_element()
    }

    pub fn document_id(&self) -> &str {
        &self.document_id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_name(&mut self, name: String, cx: &mut Context<Self>) {
        if self.name != name {
            self.name = name;
            self.dirty = true;
            cx.emit(DocumentEditorEvent::Changed);
            cx.notify();
        }
    }

    pub fn apply_remote_name(&mut self, name: String, cx: &mut Context<Self>) {
        if self.name != name {
            self.name = name;
            cx.notify();
        }
    }

    pub fn text(&self, cx: &gpui::App) -> String {
        self.editor.read(cx).text(cx)
    }

    /// Replaces the visible document as one local edit. This is used by
    /// import/restore workflows that already have a complete Markdown value;
    /// unlike remote reconciliation it deliberately marks divergence from the
    /// last synchronized projection and emits the ordinary autosave event.
    pub fn replace_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).text(cx) == text {
            return;
        }
        self.applying_remote = true;
        self.editor
            .update(cx, |editor, cx| editor.set_text(text.clone(), window, cx));
        self.applying_remote = false;
        self.refresh_document_views(&text, cx);
        self.dirty = text != self.last_synced_text;
        self.sync_rich_inline_editor(window, cx);
        cx.emit(DocumentEditorEvent::Changed);
        cx.notify();
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.last_synced_text = self.editor.read(cx).text(cx);
        self.remote_conflict = None;
        self.dirty = false;
        cx.notify();
    }

    pub fn lexical_source(&self) -> Option<&Value> {
        self.lexical_source.as_ref()
    }

    pub fn remote_conflict(&self) -> Option<&DocumentMergeConflict> {
        self.remote_conflict.as_ref()
    }

    pub fn pinned_property_ids(&self) -> Vec<String> {
        self.lexical_source
            .as_ref()
            .map(pinned_property_ids)
            .unwrap_or_default()
    }

    pub fn toggle_pinned_property(&mut self, property_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.lexical_source.as_mut() else {
            return false;
        };
        let mut property_ids = pinned_property_ids(state);
        if let Some(index) = property_ids.iter().position(|id| id == property_id) {
            property_ids.remove(index);
        } else {
            property_ids.push(property_id.to_owned());
        }
        set_pinned_property_ids(state, property_ids);
        self.dirty = true;
        cx.emit(DocumentEditorEvent::Changed);
        cx.notify();
        true
    }

    pub fn mark_saved_with_lexical_state(&mut self, state: Value, cx: &mut Context<Self>) {
        self.lexical_source = Some(state);
        self.refresh_analysis(cx);
        self.mark_saved(cx);
    }

    /// Acknowledge the exact projection that was durably queued. If the user
    /// typed again while the request was in flight, the newer edit remains
    /// dirty and will be sent by the next autosync generation instead of being
    /// incorrectly marked as saved.
    pub fn acknowledge_saved_projection(
        &mut self,
        saved_text: &str,
        lexical_state: Option<Value>,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(state) = lexical_state {
            self.lexical_source = Some(state);
            self.refresh_analysis(cx);
        }
        self.last_synced_text = saved_text.to_owned();
        self.remote_conflict = None;
        let unchanged = self.editor.read(cx).text(cx) == saved_text;
        self.dirty = !unchanged;
        cx.notify();
        unchanged
    }

    /// Applies a server/CRDT reconciliation without turning it into a local
    /// dirty edit. The Zed editor still owns selection and input state.
    pub fn apply_remote_text(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.read(cx).text(cx) == text {
            self.last_synced_text = text;
            self.remote_conflict = None;
            return;
        }
        self.applying_remote = true;
        self.editor
            .update(cx, |editor, cx| editor.set_text(text.clone(), window, cx));
        self.applying_remote = false;
        self.sync_rich_inline_editor(window, cx);
        self.last_synced_text = text;
        self.remote_conflict = None;
        self.dirty = false;
        cx.notify();
    }

    /// Refreshes the lossless Lexical source even while local Markdown is
    /// dirty. A clean editor also adopts the corresponding visible projection;
    /// a dirty editor keeps its local text and will merge it against this latest
    /// source when saved.
    pub fn apply_remote_lexical_state(
        &mut self,
        state: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let markdown = lexical_state_to_markdown(&state);
        self.lexical_source = Some(state);
        self.refresh_analysis(cx);
        if !self.dirty {
            self.apply_remote_text(markdown, window, cx);
        } else {
            let local = self.editor.read(cx).text(cx);
            match merge_document_text(&self.last_synced_text, &local, &markdown) {
                Ok(merged) => {
                    if merged != local {
                        self.applying_remote = true;
                        self.editor
                            .update(cx, |editor, cx| editor.set_text(merged.clone(), window, cx));
                        self.applying_remote = false;
                        self.sync_rich_inline_editor(window, cx);
                    }
                    self.dirty = merged != markdown;
                    self.last_synced_text = markdown;
                    self.remote_conflict = None;
                    cx.emit(DocumentEditorEvent::Changed);
                    cx.notify();
                }
                Err(conflict_markers) => {
                    // Freeze the exact local snapshot represented by the
                    // conflict banner. Leaving the inline editor active would
                    // allow newer keystrokes that Keep Local/Review Markers do
                    // not contain, turning conflict resolution into data loss.
                    self.finish_rich_inline_edit(window, cx);
                    self.last_synced_text = markdown.clone();
                    self.remote_conflict = Some(DocumentMergeConflict {
                        local,
                        remote: markdown,
                        conflict_markers,
                    });
                    cx.emit(DocumentEditorEvent::Changed);
                    cx.notify();
                }
            }
        }
    }

    pub fn resolve_remote_conflict(
        &mut self,
        resolution: DocumentConflictResolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(conflict) = self.remote_conflict.take() else {
            return false;
        };
        let replacement = match resolution {
            DocumentConflictResolution::KeepLocal => None,
            DocumentConflictResolution::UseRemote => Some((conflict.remote, false)),
            DocumentConflictResolution::ReviewMarkers => Some((conflict.conflict_markers, true)),
        };
        if let Some((replacement, dirty)) = replacement {
            self.applying_remote = true;
            self.editor
                .update(cx, |editor, cx| editor.set_text(replacement, window, cx));
            self.applying_remote = false;
            self.sync_rich_inline_editor(window, cx);
            self.dirty = dirty;
        } else {
            self.dirty = true;
        }
        cx.emit(DocumentEditorEvent::Changed);
        cx.notify();
        true
    }

    pub fn editor(&self) -> &Entity<Editor> {
        &self.editor
    }

    pub fn preview_enabled(&self) -> bool {
        self.preview_enabled
    }

    pub fn inline_block_editing(&self) -> bool {
        self.rich_inline_block_id.is_some()
    }

    /// Starts block-scoped editing while keeping the rendered document surface
    /// visible. The inline Zed editor writes through to the authoritative full
    /// document buffer, so collaboration, undo, autosave, and projection use
    /// the same event path as source-mode edits.
    pub fn begin_inline_block_edit(
        &mut self,
        block_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(position) = self
            .rich_blocks
            .iter()
            .position(|block| block.index == block_index)
        else {
            return false;
        };
        self.begin_rich_inline_edit(position, None, window, cx)
    }

    pub fn set_preview_enabled(
        &mut self,
        enabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.preview_enabled == enabled {
            return;
        }
        if !enabled {
            self.rich_inline_block_id = None;
        }
        if enabled
            && let Some(active_block) = self.active_block_index(cx)
            && let Some(position) = self
                .rich_blocks
                .iter()
                .position(|block| block.index == active_block)
        {
            self.rich_block_cursor = position;
        }
        self.preview_enabled = enabled;
        if enabled {
            if !self.rich_blocks.is_empty() {
                self.rich_block_list
                    .scroll_to_reveal_item(self.rich_block_cursor);
            }
            self.preview_focus_handle.focus(window, cx);
        } else {
            self.editor.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Converts the newest Zed selection to Macro's Lexical node coordinate
    /// space. The application then turns these points into Loro relative
    /// cursors without making the editor depend on the sync transport.
    pub fn lexical_selection(
        &self,
        cx: &mut gpui::App,
    ) -> Option<(LexicalTextPoint, LexicalTextPoint)> {
        let state = self.lexical_source.as_ref()?;
        let selection = self
            .editor
            .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
            .into_iter()
            .last()?;
        let anchor = markdown_offset_to_lexical_point(state, selection.start)?;
        let focus = markdown_offset_to_lexical_point(state, selection.end)?;
        Some((anchor, focus))
    }

    pub fn selected_markdown_range(&self, cx: &mut gpui::App) -> Option<Range<usize>> {
        self.selected_markdown_ranges(cx).into_iter().last()
    }

    pub fn selected_markdown_ranges(&self, cx: &mut gpui::App) -> Vec<Range<usize>> {
        self.editor
            .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
    }

    pub fn clipboard_markdown(&self, cx: &mut gpui::App) -> String {
        let text = self.text(cx);
        let Some(range) = self.selected_markdown_range(cx) else {
            return text;
        };
        let start = floor_char_boundary(&text, range.start.min(text.len()));
        let end = floor_char_boundary(&text, range.end.min(text.len()));
        if start == end {
            text
        } else {
            text[start.min(end)..start.max(end)].to_owned()
        }
    }

    pub fn replace_selection_with_markdown(
        &mut self,
        markdown: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let text = self.text(cx);
        let range = self
            .selected_markdown_range(cx)
            .unwrap_or(text.len()..text.len());
        let start = floor_char_boundary(&text, range.start.min(text.len()));
        let end = floor_char_boundary(&text, range.end.min(text.len()));
        let range = start.min(end)..start.max(end);
        let caret_offset = range.start + markdown.len();
        let mut updated = text;
        updated.replace_range(range.clone(), &markdown);
        let caret = point_for_offset(&updated, caret_offset);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(range.start)..MultiBufferOffset(range.end),
                    markdown,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        true
    }

    pub fn accept_lexical_annotation_state(&mut self, state: Value, cx: &mut Context<Self>) {
        self.lexical_source = Some(state);
        self.refresh_analysis(cx);
        cx.notify();
    }

    pub fn remote_selections(&self) -> &[RemoteDocumentSelection] {
        &self.remote_selections
    }

    /// Paint remote Macro awareness ranges directly in Zed's editor surface.
    /// Empty ranges are expanded by one visible character so a remote caret is
    /// still discoverable; participant colors come from the active Zed theme.
    pub fn set_remote_selections(
        &mut self,
        mut selections: Vec<RemoteDocumentSelection>,
        cx: &mut Context<Self>,
    ) {
        selections.sort_by_key(|selection| {
            (
                selection.anchor.min(selection.focus),
                selection.anchor.max(selection.focus),
                selection.peer_id.clone(),
            )
        });
        self.remote_selections = selections.clone();
        let text = self.editor.read(cx).text(cx);
        self.editor.update(cx, |editor, cx| {
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let mut ranges = Vec::with_capacity(selections.len());
            let mut color_indices = Vec::with_capacity(selections.len());
            for selection in selections {
                let mut start = selection.anchor.min(selection.focus).min(text.len());
                let mut end = selection.anchor.max(selection.focus).min(text.len());
                start = floor_char_boundary(&text, start);
                end = floor_char_boundary(&text, end);
                if start == end {
                    if end < text.len() {
                        end = next_char_boundary(&text, end);
                    } else if start > 0 {
                        start = previous_char_boundary(&text, start);
                    }
                }
                ranges.push(
                    snapshot.anchor_before(MultiBufferOffset(start))
                        ..snapshot.anchor_after(MultiBufferOffset(end)),
                );
                color_indices.push(selection.color_index);
            }
            let key = HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<
                MacroRemoteSelectionHighlights,
            >());
            if ranges.is_empty() {
                editor.clear_background_highlights(key, cx);
            } else {
                editor.highlight_background(
                    key,
                    &ranges,
                    move |index, theme| {
                        theme
                            .players()
                            .color_for_participant(color_indices[*index])
                            .selection
                    },
                    cx,
                );
            }
        });
        cx.notify();
    }

    pub fn stats(&self, cx: &gpui::App) -> DocumentStats {
        let _ = cx;
        self.analysis.stats
    }

    pub fn outline(&self, cx: &gpui::App) -> Vec<OutlineItem> {
        let _ = cx;
        self.analysis.outline.clone()
    }

    pub fn block_summaries(&self, cx: &gpui::App) -> Vec<DocumentBlockSummary> {
        let _ = cx;
        self.analysis.blocks.clone()
    }

    pub fn analysis(&self, cx: &gpui::App) -> DocumentAnalysis {
        let _ = cx;
        self.analysis.clone()
    }

    fn refresh_analysis(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.refresh_document_views(&text, cx);
    }

    pub fn active_block_index(&self, cx: &mut gpui::App) -> Option<usize> {
        if self.preview_enabled {
            return self
                .rich_blocks
                .get(self.rich_block_cursor)
                .map(|block| block.index);
        }
        let text = self.text(cx);
        let selection = self.selected_markdown_range(cx)?;
        let offset = selection.start.min(text.len());
        let blocks = markdown_block_ranges(&text);
        blocks.iter().enumerate().position(|(index, block)| {
            (block.start..block.end).contains(&offset)
                || (index + 1 == blocks.len() && offset == block.end)
        })
    }

    /// Selects an entire visible Markdown block. The side-panel block rail and
    /// keyboard editing surface therefore share one real Zed editor selection
    /// instead of maintaining a second visual-only selection model.
    pub fn select_block(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = self.text(cx);
        let Some(range) = markdown_block_ranges(&text).get(index).cloned() else {
            return false;
        };
        let anchor = point_for_offset(&text, range.start);
        let focus = point_for_offset(&text, range.end);
        if let Some(position) = self
            .rich_blocks
            .iter()
            .position(|block| block.index == index)
        {
            self.rich_block_cursor = position;
            self.rich_block_list.scroll_to_reveal_item(position);
        }
        let preview_enabled = self.preview_enabled;
        let preview_focus_handle = self.preview_focus_handle.clone();
        self.editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([anchor..focus]),
            );
            if !preview_enabled {
                editor.focus_handle(cx).focus(window, cx);
            }
        });
        if preview_enabled {
            preview_focus_handle.focus(window, cx);
        }
        cx.notify();
        true
    }

    pub fn apply_structure_command_to_block(
        &mut self,
        index: usize,
        command: DocumentStructureCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || !self.select_block(index, window, cx) {
            return false;
        }
        self.apply_structure_command(command, window, cx)
    }

    pub fn go_to_line(&mut self, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([Point::new(line, 0)..Point::new(line, 0)]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
    }

    /// Places the native caret at a UTF-8 byte offset. Import, restore, and
    /// deterministic visual workflows use this when they already have a
    /// document-relative position rather than a line number.
    pub fn set_caret_offset(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text(cx);
        let point = point_for_offset(&text, floor_char_boundary(&text, offset.min(text.len())));
        self.editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([point..point]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        cx.notify();
    }

    pub fn slash_command_context(&self, cx: &mut gpui::App) -> Option<DocumentSlashContext> {
        let (text, selection) = if self.rich_inline_block_id.is_some() {
            let text = self.rich_inline_editor.read(cx).text(cx);
            let selection = self
                .rich_inline_editor
                .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
                .into_iter()
                .last()?;
            (text, selection)
        } else {
            (self.text(cx), self.selected_markdown_range(cx)?)
        };
        if selection.start != selection.end {
            return None;
        }
        document_slash_context(&text, selection.end)
    }

    fn raw_snippet_context(&self, cx: &mut gpui::App) -> Option<DocumentSnippetContext> {
        if self.rich_inline_block_id.is_some() {
            let inline_text = self.rich_inline_editor.read(cx).text(cx);
            let selection = self
                .rich_inline_editor
                .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
                .into_iter()
                .last()?;
            if selection.start != selection.end {
                return None;
            }
            let mut context = document_snippet_context(&inline_text, selection.end)?;
            let stable_id = self.rich_inline_block_id.as_deref()?;
            let block_index = self
                .rich_blocks
                .iter()
                .find(|block| block.stable_id == stable_id)?
                .index;
            let document_text = self.text(cx);
            let block = markdown_block_ranges(&document_text)
                .get(block_index)?
                .clone();
            if context.range.end > block.end.saturating_sub(block.start) {
                return None;
            }
            context.range = (block.start + context.range.start)..(block.start + context.range.end);
            return Some(context);
        }

        let text = self.text(cx);
        let selection = self.selected_markdown_range(cx)?;
        if selection.start != selection.end {
            return None;
        }
        document_snippet_context(&text, selection.end)
    }

    pub fn snippet_context(&self, cx: &mut gpui::App) -> Option<DocumentSnippetContext> {
        let context = self.raw_snippet_context(cx)?;
        (self.dismissed_snippet_context.as_ref() != Some(&context)).then_some(context)
    }

    fn snippet_typeahead_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return false;
        }

        let raw_context = self.raw_snippet_context(cx);
        if self.dismissed_snippet_context.is_some()
            && self.dismissed_snippet_context.as_ref() != raw_context.as_ref()
        {
            // Escape dismisses only the exact trigger that was visible. Any
            // subsequent edit or caret movement is a fresh typeahead session.
            self.dismissed_snippet_context = None;
        }
        let Some(context) = raw_context else {
            return false;
        };
        if self.dismissed_snippet_context.as_ref() == Some(&context) {
            return false;
        }

        let Some(command) = snippet_typeahead_command(event.keystroke.key.as_str(), false) else {
            return false;
        };
        if command == SnippetTypeaheadCommand::Dismiss {
            self.dismissed_snippet_context = Some(context);
        }
        cx.emit(DocumentEditorEvent::SnippetTypeahead(command));
        cx.stop_propagation();
        cx.notify();
        true
    }

    fn source_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.snippet_typeahead_key_down(event, cx);
    }

    pub fn insert_snippet_state(
        &mut self,
        snippet_state: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.read_only {
            return Err("this document is read only".into());
        }
        let context = self
            .snippet_context(cx)
            .ok_or_else(|| "the snippet trigger is no longer active".to_owned())?;
        let current = self.text(cx);
        let insertion = insert_snippet_lexical_state(
            self.lexical_source.as_ref(),
            &current,
            context.range,
            &snippet_state,
        )?;
        let was_inline_editing = self.rich_inline_block_id.is_some();
        if was_inline_editing {
            self.finish_rich_inline_edit(window, cx);
        }
        self.lexical_source = Some(insertion.state);
        self.applying_remote = true;
        let caret = point_for_offset(&insertion.markdown, insertion.caret);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(current.len()),
                    insertion.markdown.clone(),
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
            if !was_inline_editing {
                editor.focus_handle(cx).focus(window, cx);
            }
        });
        self.applying_remote = false;
        self.refresh_document_views(&insertion.markdown, cx);
        self.dirty = insertion.markdown != self.last_synced_text;
        if was_inline_editing {
            self.preview_focus_handle.focus(window, cx);
        }
        cx.emit(DocumentEditorEvent::Changed);
        cx.notify();
        Ok(())
    }

    pub fn apply_slash_command(
        &mut self,
        command: MarkdownCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        let Some(context) = self.slash_command_context(cx) else {
            return false;
        };
        if self.rich_inline_block_id.is_some() {
            let text = self.rich_inline_editor.read(cx).text(cx);
            let mut updated = text.clone();
            updated.replace_range(context.range.clone(), "");
            let command_edit =
                markdown_command_edit(&updated, context.range.start..context.range.start, command);
            updated.replace_range(command_edit.range.clone(), &command_edit.replacement);
            let anchor = point_for_offset(&updated, command_edit.selection.start);
            let focus = point_for_offset(&updated, command_edit.selection.end);
            self.rich_inline_editor.update(cx, |editor, cx| {
                editor.edit(
                    [(MultiBufferOffset(0)..MultiBufferOffset(text.len()), updated)],
                    cx,
                );
                editor.change_selections(
                    SelectionEffects::scroll(Autoscroll::fit()),
                    window,
                    cx,
                    |selections| selections.select_ranges([anchor..focus]),
                );
                editor.focus_handle(cx).focus(window, cx);
            });
            return true;
        }
        let caret_text = self.text(cx);
        let caret = point_for_offset(&caret_text, context.range.start);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(context.range.start)..MultiBufferOffset(context.range.end),
                    String::new(),
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
        });
        self.apply_markdown_command(command, window, cx)
    }

    pub fn insert_macro_mention(
        &mut self,
        mention: MacroInlineMention,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), MentionInsertionError> {
        if self.read_only {
            return Err(MentionInsertionError::SelectionDoesNotResolveToText);
        }
        let current = self.text(cx);
        let selection = self
            .selected_markdown_range(cx)
            .unwrap_or(current.len()..current.len());
        if selection.start != selection.end {
            return Err(MentionInsertionError::SelectionNotCollapsed);
        }
        let source = match self.lexical_source.as_ref() {
            Some(source) => project_markdown_edits(source, &current)
                .map_err(MentionInsertionError::ProtectedSchema)?,
            None => markdown_to_lexical_state(&current),
        };
        let (updated_source, caret_offset) =
            insert_inline_mention(&source, selection.start, &mention)?;
        let updated_text = lexical_state_to_markdown(&updated_source);
        let caret = point_for_offset(&updated_text, caret_offset);
        self.lexical_source = Some(updated_source);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(current.len()),
                    updated_text,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        Ok(())
    }

    pub fn select_document_search_match(
        &mut self,
        query: &str,
        options: DocumentSearchOptions,
        backwards: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Option<Range<usize>>, String> {
        let text = self.text(cx);
        let matches = document_search_matches(&text, query, options)?;
        if matches.is_empty() {
            return Ok(None);
        }
        let selection = self
            .selected_markdown_range(cx)
            .unwrap_or(text.len()..text.len());
        let exact_index = matches.iter().position(|range| *range == selection);
        let target = if backwards {
            exact_index
                .map(|index| {
                    matches[(index + matches.len().saturating_sub(1)) % matches.len()].clone()
                })
                .or_else(|| {
                    matches
                        .iter()
                        .rev()
                        .find(|range| range.end <= selection.start)
                        .cloned()
                })
                .unwrap_or_else(|| matches.last().expect("matches is non-empty").clone())
        } else {
            exact_index
                .map(|index| matches[(index + 1) % matches.len()].clone())
                .or_else(|| {
                    matches
                        .iter()
                        .find(|range| range.start >= selection.end)
                        .cloned()
                })
                .unwrap_or_else(|| matches[0].clone())
        };
        let anchor = point_for_offset(&text, target.start);
        let focus = point_for_offset(&text, target.end);
        self.editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([anchor..focus]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        Ok(Some(target))
    }

    pub fn replace_current_document_search_match(
        &mut self,
        query: &str,
        replacement: &str,
        options: DocumentSearchOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        if self.read_only {
            return Err("This document is read only".into());
        }
        let text = self.text(cx);
        let matches = document_search_matches(&text, query, options)?;
        if matches.is_empty() {
            return Ok(false);
        }
        let selection = self
            .selected_markdown_range(cx)
            .unwrap_or(text.len()..text.len());
        let target = matches
            .iter()
            .find(|range| **range == selection)
            .cloned()
            .or_else(|| {
                matches
                    .iter()
                    .find(|range| range.start >= selection.end)
                    .cloned()
            })
            .unwrap_or_else(|| matches[0].clone());
        let replacement =
            replacement_for_document_match(&text, query, replacement, options, &target)?;
        let caret_offset = target.start + replacement.len();
        let mut updated = text;
        updated.replace_range(target.clone(), &replacement);
        let caret = point_for_offset(&updated, caret_offset);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(target.start)..MultiBufferOffset(target.end),
                    replacement,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        Ok(true)
    }

    pub fn replace_all_document_search_matches(
        &mut self,
        query: &str,
        replacement: &str,
        options: DocumentSearchOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<usize, String> {
        if self.read_only {
            return Err("This document is read only".into());
        }
        let current = self.text(cx);
        let result = replace_all_document_matches(&current, query, replacement, options)?;
        if result.replacements == 0 {
            return Ok(0);
        }
        let caret = point_for_offset(&result.text, result.text.len());
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(current.len()),
                    result.text,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([caret..caret]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        Ok(result.replacements)
    }

    pub fn apply_structure_command(
        &mut self,
        command: DocumentStructureCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if self.rich_inline_block_id.is_some() {
            match command {
                DocumentStructureCommand::Indent | DocumentStructureCommand::Outdent => {
                    let text = self.rich_inline_editor.read(cx).text(cx);
                    let selection = self
                        .rich_inline_editor
                        .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
                        .into_iter()
                        .last()
                        .unwrap_or(text.len()..text.len());
                    let Some(edit) = markdown_structure_command_edit(&text, selection, command)
                    else {
                        return false;
                    };
                    let mut updated = text;
                    updated.replace_range(edit.range.clone(), &edit.replacement);
                    let anchor = point_for_offset(&updated, edit.selection.start);
                    let focus = point_for_offset(&updated, edit.selection.end);
                    self.rich_inline_editor.update(cx, |editor, cx| {
                        editor.edit(
                            [(
                                MultiBufferOffset(edit.range.start)
                                    ..MultiBufferOffset(edit.range.end),
                                edit.replacement,
                            )],
                            cx,
                        );
                        editor.change_selections(
                            SelectionEffects::scroll(Autoscroll::fit()),
                            window,
                            cx,
                            |selections| selections.select_ranges([anchor..focus]),
                        );
                        editor.focus_handle(cx).focus(window, cx);
                    });
                    return true;
                }
                DocumentStructureCommand::MoveBlockUp | DocumentStructureCommand::MoveBlockDown => {
                    let Some(block_index) = self
                        .rich_blocks
                        .get(self.rich_block_cursor)
                        .map(|block| block.index)
                    else {
                        return false;
                    };
                    self.finish_rich_inline_edit(window, cx);
                    if !self.select_block(block_index, window, cx) {
                        return false;
                    }
                }
            }
        }
        let text = self.text(cx);
        let selection = self
            .selected_markdown_range(cx)
            .unwrap_or(text.len()..text.len());
        let Some(edit) = markdown_structure_command_edit(&text, selection, command) else {
            return false;
        };
        let mut updated = text;
        updated.replace_range(edit.range.clone(), &edit.replacement);
        let anchor = point_for_offset(&updated, edit.selection.start);
        let focus = point_for_offset(&updated, edit.selection.end);
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(edit.range.start)..MultiBufferOffset(edit.range.end),
                    edit.replacement,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([anchor..focus]),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        true
    }

    pub fn move_block_to(
        &mut self,
        source_index: usize,
        target_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only || source_index == target_index {
            return false;
        }
        let original = self.text(cx);
        let Some(edit) = markdown_move_block_to_edit(&original, source_index, target_index) else {
            return false;
        };
        let updated = edit.replacement;
        let selection = edit.selection;
        if let Some(source) = self.lexical_source.as_ref()
            && project_markdown_edits(source, &updated).is_err()
        {
            return false;
        }
        let anchor = point_for_offset(&updated, selection.start);
        let focus = point_for_offset(&updated, selection.end);
        let preview_enabled = self.preview_enabled;
        self.editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(original.len()),
                    updated,
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| selections.select_ranges([anchor..focus]),
            );
            if !preview_enabled {
                editor.focus_handle(cx).focus(window, cx);
            }
        });
        if preview_enabled {
            self.preview_focus_handle.focus(window, cx);
        }
        true
    }

    pub fn apply_markdown_command(
        &mut self,
        command: MarkdownCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.read_only {
            return false;
        }
        if self.rich_inline_block_id.is_some() {
            let text = self.rich_inline_editor.read(cx).text(cx);
            let selections = self
                .rich_inline_editor
                .update(cx, |editor, cx| editor.selection_offset_ranges(cx))
                .into_iter()
                .collect::<Vec<_>>();
            let Some(edit) = markdown_multi_command_edit(&text, &selections, command) else {
                return false;
            };
            let buffer_edits = edit
                .edits
                .iter()
                .map(|edit| {
                    (
                        MultiBufferOffset(edit.range.start)..MultiBufferOffset(edit.range.end),
                        edit.replacement.clone(),
                    )
                })
                .collect::<Vec<_>>();
            let selection_ranges = edit
                .selections
                .iter()
                .map(|range| {
                    point_for_offset(&edit.text, range.start)
                        ..point_for_offset(&edit.text, range.end)
                })
                .collect::<Vec<_>>();
            self.rich_inline_editor.update(cx, |editor, cx| {
                editor.edit(buffer_edits, cx);
                editor.change_selections(
                    SelectionEffects::scroll(Autoscroll::fit()),
                    window,
                    cx,
                    |selections| selections.select_ranges(selection_ranges),
                );
                editor.focus_handle(cx).focus(window, cx);
            });
            return true;
        }
        let text = self.text(cx);
        let selections = self.selected_markdown_ranges(cx);
        let Some(edit) = markdown_multi_command_edit(&text, &selections, command) else {
            return false;
        };
        let buffer_edits = edit
            .edits
            .iter()
            .map(|edit| {
                (
                    MultiBufferOffset(edit.range.start)..MultiBufferOffset(edit.range.end),
                    edit.replacement.clone(),
                )
            })
            .collect::<Vec<_>>();
        let selection_ranges = edit
            .selections
            .iter()
            .map(|range| {
                point_for_offset(&edit.text, range.start)..point_for_offset(&edit.text, range.end)
            })
            .collect::<Vec<_>>();
        self.editor.update(cx, |editor, cx| {
            editor.edit(buffer_edits, cx);
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges(selection_ranges),
            );
            editor.focus_handle(cx).focus(window, cx);
        });
        true
    }
}

fn point_for_offset(text: &str, offset: usize) -> Point {
    let offset = floor_char_boundary(text, offset);
    let prefix = &text[..offset];
    let row = prefix.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let column = prefix
        .rfind('\n')
        .map_or(prefix.len(), |index| prefix.len() - index - 1) as u32;
    Point::new(row, column)
}

fn line_range(text: &str, selection: Range<usize>) -> Range<usize> {
    let start = floor_char_boundary(text, selection.start.min(text.len()));
    let end = floor_char_boundary(text, selection.end.min(text.len()));
    let start = text[..start].rfind('\n').map_or(0, |index| index + 1);
    let end = text[end..]
        .find('\n')
        .map_or(text.len(), |index| end + index);
    start..end
}

fn markdown_block_ranges(text: &str) -> Vec<Range<usize>> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let mut parsed_ranges = Vec::new();
    let mut block_depth = 0usize;
    let mut block_start = None;
    for (event, source_range) in
        Parser::new_ext(text, document_markdown_options()).into_offset_iter()
    {
        match event {
            Event::Start(tag) if is_markdown_block_tag(&tag) => {
                if block_depth == 0 {
                    block_start = Some(source_range.start);
                }
                block_depth += 1;
            }
            Event::End(tag) if is_markdown_block_tag_end(tag) => {
                if block_depth == 0 {
                    continue;
                }
                block_depth -= 1;
                if block_depth == 0
                    && let Some(start) = block_start.take()
                {
                    push_markdown_block_range(text, &mut parsed_ranges, start..source_range.end);
                }
            }
            Event::Rule | Event::DisplayMath(_) if block_depth == 0 => {
                push_markdown_block_range(text, &mut parsed_ranges, source_range);
            }
            _ => {}
        }
    }

    // Reference definitions and future Markdown constructs can be intentionally
    // omitted from the event stream. Preserve every non-whitespace source gap
    // as a block so navigation and reordering can never silently drop it.
    parsed_ranges.sort_by_key(|range| range.start);
    let mut ranges = Vec::with_capacity(parsed_ranges.len());
    let mut cursor = 0;
    for range in parsed_ranges {
        push_markdown_block_range(text, &mut ranges, cursor..range.start);
        ranges.push(range.clone());
        cursor = range.end;
    }
    push_markdown_block_range(text, &mut ranges, cursor..text.len());
    ranges
}

fn document_markdown_options() -> Options {
    let mut options = Options::all();
    options.remove(Options::ENABLE_SMART_PUNCTUATION);
    options
}

fn is_markdown_block_tag(tag: &Tag<'_>) -> bool {
    matches!(
        tag,
        Tag::Paragraph
            | Tag::Heading { .. }
            | Tag::BlockQuote(_)
            | Tag::CodeBlock(_)
            | Tag::HtmlBlock
            | Tag::List(_)
            | Tag::Item
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Table(_)
            | Tag::TableHead
            | Tag::TableRow
            | Tag::TableCell
            | Tag::MetadataBlock(_)
    )
}

fn is_markdown_block_tag_end(tag: TagEnd) -> bool {
    matches!(
        tag,
        TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::BlockQuote(_)
            | TagEnd::CodeBlock
            | TagEnd::HtmlBlock
            | TagEnd::List(_)
            | TagEnd::Item
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Table
            | TagEnd::TableHead
            | TagEnd::TableRow
            | TagEnd::TableCell
            | TagEnd::MetadataBlock(_)
    )
}

fn push_markdown_block_range(text: &str, ranges: &mut Vec<Range<usize>>, mut range: Range<usize>) {
    range.start = range.start.min(text.len());
    range.end = range.end.min(text.len()).max(range.start);
    while range.start < range.end {
        let Some(character) = text[range.start..range.end].chars().next() else {
            break;
        };
        if !matches!(character, '\r' | '\n') {
            break;
        }
        range.start += character.len_utf8();
    }
    while range.end > range.start {
        let Some(character) = text[range.start..range.end].chars().next_back() else {
            break;
        };
        if !matches!(character, '\r' | '\n') {
            break;
        }
        range.end -= character.len_utf8();
    }
    if range.start < range.end && !text[range.clone()].trim().is_empty() {
        ranges.push(range);
    }
}

fn markdown_block_kind(block: &str) -> DocumentBlockKind {
    let first = block
        .lines()
        .map(str::trim_start)
        .find(|line| !line.is_empty());
    let Some(first) = first else {
        return DocumentBlockKind::Paragraph;
    };
    if first.starts_with("- [ ] ") || first.starts_with("- [x] ") || first.starts_with("- [X] ") {
        DocumentBlockKind::Checklist
    } else if first.starts_with("- ") || first.starts_with("* ") || first.starts_with("+ ") {
        DocumentBlockKind::BulletedList
    } else if first.split_once(". ").is_some_and(|(prefix, _)| {
        !prefix.is_empty() && prefix.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        DocumentBlockKind::NumberedList
    } else if first.starts_with('#') {
        DocumentBlockKind::Heading
    } else if first.starts_with('>') {
        DocumentBlockKind::Quote
    } else if first.starts_with("```") || first.starts_with("~~~") {
        DocumentBlockKind::Code
    } else if first == "---" || first == "***" || first == "___" {
        DocumentBlockKind::Divider
    } else if first.starts_with('|') && block.lines().any(|line| line.contains("---")) {
        DocumentBlockKind::Table
    } else if first.starts_with("![") || first.starts_with("[Video]") {
        DocumentBlockKind::Media
    } else {
        DocumentBlockKind::Paragraph
    }
}

fn lexical_top_level_block_ids(state: &Value) -> Vec<Option<String>> {
    state
        .get("root")
        .and_then(|root| root.get("children"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|node| {
            node.get("$")
                .and_then(|metadata| metadata.get("id"))
                .and_then(Value::as_str)
                .or_else(|| node.get("id").and_then(Value::as_str))
                .or_else(|| node.get("key").and_then(Value::as_str))
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn is_positional_native_block_id(value: &str) -> bool {
    value.strip_prefix("native-block-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

pub fn markdown_block_summaries_with_source(
    text: &str,
    lexical_source: Option<&Value>,
) -> Vec<DocumentBlockSummary> {
    let ranges = markdown_block_ranges(text);
    let mut lexical_ids = lexical_source
        .map(lexical_top_level_block_ids)
        .unwrap_or_default();
    if lexical_ids.len() != ranges.len() {
        lexical_ids.clear();
    }
    let mut previous_block_start = 0_usize;
    let mut line = 0_u32;
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, range)| {
            // Block ranges are produced in document order. Advancing from the
            // previous start counts each byte once; rescanning `text[..start]`
            // for every block made large documents quadratic and dominated
            // native editor initialization.
            line += text[previous_block_start..range.start]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count() as u32;
            previous_block_start = range.start;
            let block = &text[range.clone()];
            let preview = block
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("Empty block")
                .trim_start_matches(|character: char| {
                    matches!(
                        character,
                        '#' | '>' | '-' | '*' | '+' | '[' | ']' | '`' | ' '
                    ) || character.is_ascii_digit()
                        || character == '.'
                })
                .trim()
                .chars()
                .take(80)
                .collect::<String>();
            DocumentBlockSummary {
                stable_id: lexical_ids
                    .get(index)
                    .and_then(Clone::clone)
                    .unwrap_or_else(|| format!("native-block-{index}")),
                index,
                line,
                kind: markdown_block_kind(block),
                range,
                preview: if preview.is_empty() {
                    "Untitled block".into()
                } else {
                    preview
                },
            }
        })
        .collect()
}

pub fn markdown_block_summaries(text: &str) -> Vec<DocumentBlockSummary> {
    markdown_block_summaries_with_source(text, None)
}

pub fn markdown_move_block_to_edit(
    text: &str,
    source_index: usize,
    target_index: usize,
) -> Option<MarkdownCommandEdit> {
    if source_index == target_index {
        return None;
    }
    let ranges = markdown_block_ranges(text);
    let mut selection = ranges.get(source_index)?.clone();
    if target_index >= ranges.len() {
        return None;
    }
    let mut updated = text.to_owned();
    let mut index = source_index;
    while index != target_index {
        let command = if index < target_index {
            DocumentStructureCommand::MoveBlockDown
        } else {
            DocumentStructureCommand::MoveBlockUp
        };
        let edit = markdown_structure_command_edit(&updated, selection, command)?;
        updated.replace_range(edit.range, &edit.replacement);
        selection = edit.selection;
        if index < target_index {
            index += 1;
        } else {
            index -= 1;
        }
    }
    Some(MarkdownCommandEdit {
        range: 0..text.len(),
        replacement: updated,
        selection,
    })
}

fn markdown_block_move_edit(
    text: &str,
    selection: Range<usize>,
    upwards: bool,
) -> Option<MarkdownCommandEdit> {
    let blocks = markdown_block_ranges(text);
    if blocks.len() < 2 {
        return None;
    }
    let start = floor_char_boundary(text, selection.start.min(text.len()));
    let end = floor_char_boundary(text, selection.end.min(text.len()));
    let collapsed = start == end;
    let intersects = |index: usize, block: &Range<usize>| {
        if collapsed {
            (block.start..block.end).contains(&start)
                || (index + 1 == blocks.len() && start == block.end)
        } else {
            block.start < end && block.end > start
        }
    };
    let first = blocks
        .iter()
        .enumerate()
        .position(|(index, block)| intersects(index, block))?;
    let last = blocks
        .iter()
        .enumerate()
        .rposition(|(index, block)| intersects(index, block))?;
    let selected_start = blocks[first].start;
    let selected_end = blocks[last].end;
    let selection_start_in_group = start.saturating_sub(selected_start);
    let selection_end_in_group = end.saturating_sub(selected_start);
    let selected = &text[selected_start..selected_end];

    if upwards {
        let previous = first.checked_sub(1)?;
        let previous_range = blocks[previous].clone();
        let previous_text = &text[previous_range.clone()];
        let separator = &text[previous_range.end..selected_start];
        Some(MarkdownCommandEdit {
            range: previous_range.start..selected_end,
            replacement: format!("{selected}{separator}{previous_text}"),
            selection: (previous_range.start + selection_start_in_group)
                ..(previous_range.start + selection_end_in_group),
        })
    } else {
        let next_range = blocks.get(last + 1)?.clone();
        let next_text = &text[next_range.clone()];
        let separator = &text[selected_end..next_range.start];
        let moved_start = selected_start + next_text.len() + separator.len();
        Some(MarkdownCommandEdit {
            range: selected_start..next_range.end,
            replacement: format!("{next_text}{separator}{selected}"),
            selection: (moved_start + selection_start_in_group)
                ..(moved_start + selection_end_in_group),
        })
    }
}

fn markdown_indent_edit(
    text: &str,
    selection: Range<usize>,
    outdent: bool,
) -> Option<MarkdownCommandEdit> {
    let range = line_range(text, selection);
    let segment = &text[range.clone()];
    let mut changed = false;
    let replacement = segment
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                return String::new();
            }
            if outdent {
                if let Some(rest) = line.strip_prefix('\t') {
                    changed = true;
                    rest.to_owned()
                } else {
                    let spaces = line
                        .bytes()
                        .take_while(|byte| *byte == b' ')
                        .take(2)
                        .count();
                    if spaces > 0 {
                        changed = true;
                        line[spaces..].to_owned()
                    } else {
                        line.to_owned()
                    }
                }
            } else {
                changed = true;
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    changed.then(|| MarkdownCommandEdit {
        range: range.clone(),
        selection: range.start..range.start + replacement.len(),
        replacement,
    })
}

pub fn markdown_structure_command_edit(
    text: &str,
    selection: Range<usize>,
    command: DocumentStructureCommand,
) -> Option<MarkdownCommandEdit> {
    match command {
        DocumentStructureCommand::Indent => markdown_indent_edit(text, selection, false),
        DocumentStructureCommand::Outdent => markdown_indent_edit(text, selection, true),
        DocumentStructureCommand::MoveBlockUp => markdown_block_move_edit(text, selection, true),
        DocumentStructureCommand::MoveBlockDown => markdown_block_move_edit(text, selection, false),
    }
}

fn strip_markdown_block_prefix(line: &str) -> &str {
    let trimmed = line.trim_start();
    if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("- [x] ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("- [X] ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("- ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("* ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("+ ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("> ") {
        rest
    } else {
        let heading = trimmed.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=6).contains(&heading) && trimmed.as_bytes().get(heading) == Some(&b' ') {
            &trimmed[heading + 1..]
        } else if let Some((digits, rest)) = trimmed.split_once(". ") {
            if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
                rest
            } else {
                trimmed
            }
        } else {
            trimmed
        }
    }
}

fn block_command_prefix(command: MarkdownCommand, index: usize) -> Option<String> {
    match command {
        MarkdownCommand::Paragraph => Some(String::new()),
        MarkdownCommand::Heading(level) => {
            Some(format!("{} ", "#".repeat(level.clamp(1, 6) as usize)))
        }
        MarkdownCommand::BulletList => Some("- ".into()),
        MarkdownCommand::NumberedList => Some(format!("{}. ", index + 1)),
        MarkdownCommand::Checklist => Some("- [ ] ".into()),
        MarkdownCommand::Quote => Some("> ".into()),
        _ => None,
    }
}

fn wrapped_command_edit(
    text: &str,
    selection: Range<usize>,
    prefix: &str,
    suffix: &str,
    placeholder: &str,
) -> MarkdownCommandEdit {
    let start = floor_char_boundary(text, selection.start.min(text.len()));
    let end = floor_char_boundary(text, selection.end.min(text.len())).max(start);
    let selected = &text[start..end];
    let content = if selected.is_empty() {
        placeholder
    } else {
        selected
    };
    let replacement = format!("{prefix}{content}{suffix}");
    let selection = start + prefix.len()..start + prefix.len() + content.len();
    MarkdownCommandEdit {
        range: start..end,
        replacement,
        selection,
    }
}

/// Computes the smallest source edit for one native block/format command.
/// Keeping this pure makes command behavior testable independently from GPUI
/// focus and ensures replacements are always UTF-8 boundary safe.
pub fn markdown_command_edit(
    text: &str,
    selection: Range<usize>,
    command: MarkdownCommand,
) -> MarkdownCommandEdit {
    if let Some(prefix) = block_command_prefix(command, 0) {
        let range = line_range(text, selection);
        let empty_active_line = range.start == range.end;
        let replacement = text[range.clone()]
            .split('\n')
            .enumerate()
            .map(|(index, line)| {
                if line.trim().is_empty() {
                    if empty_active_line && index == 0 {
                        block_command_prefix(command, index).unwrap_or_default()
                    } else {
                        String::new()
                    }
                } else {
                    let indent_len = line.len() - line.trim_start().len();
                    let indent = &line[..indent_len];
                    let content = strip_markdown_block_prefix(line);
                    let prefix =
                        block_command_prefix(command, index).unwrap_or_else(|| prefix.clone());
                    format!("{indent}{prefix}{content}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let caret = range.start + replacement.len();
        return MarkdownCommandEdit {
            range,
            replacement,
            selection: caret..caret,
        };
    }

    match command {
        MarkdownCommand::CodeBlock => {
            wrapped_command_edit(text, selection, "```\n", "\n```", "code")
        }
        MarkdownCommand::Divider => {
            let range = line_range(text, selection);
            MarkdownCommandEdit {
                selection: range.start + 3..range.start + 3,
                range,
                replacement: "---".into(),
            }
        }
        MarkdownCommand::Table => {
            let start = floor_char_boundary(text, selection.start.min(text.len()));
            let replacement =
                "| Column 1 | Column 2 |\n| --- | --- |\n| Value | Value |".to_owned();
            MarkdownCommandEdit {
                range: start..floor_char_boundary(text, selection.end.min(text.len())).max(start),
                selection: start + 2..start + 10,
                replacement,
            }
        }
        MarkdownCommand::Bold => wrapped_command_edit(text, selection, "**", "**", "bold"),
        MarkdownCommand::Italic => wrapped_command_edit(text, selection, "_", "_", "italic"),
        MarkdownCommand::Strikethrough => {
            wrapped_command_edit(text, selection, "~~", "~~", "strikethrough")
        }
        MarkdownCommand::InlineCode => wrapped_command_edit(text, selection, "`", "`", "code"),
        MarkdownCommand::Link | MarkdownCommand::Image => {
            let image = command == MarkdownCommand::Image;
            let mut edit = wrapped_command_edit(
                text,
                selection,
                if image { "![" } else { "[" },
                "](https://)",
                if image { "alt text" } else { "link text" },
            );
            let url_start = edit.range.start + edit.replacement.len() - "https://)".len();
            edit.selection = url_start..url_start + "https://".len();
            edit
        }
        MarkdownCommand::Video => {
            let start = floor_char_boundary(text, selection.start.min(text.len()));
            let end = floor_char_boundary(text, selection.end.min(text.len())).max(start);
            let replacement = "[Video](https://)".to_owned();
            let url_start = start + "[Video](".len();
            MarkdownCommandEdit {
                range: start..end,
                replacement,
                selection: url_start..url_start + "https://".len(),
            }
        }
        MarkdownCommand::InlineMath => wrapped_command_edit(text, selection, "$", "$", "equation"),
        MarkdownCommand::BlockMath => {
            wrapped_command_edit(text, selection, "$$\n", "\n$$", "equation")
        }
        MarkdownCommand::Superscript => {
            wrapped_command_edit(text, selection, "^", "^", "superscript")
        }
        MarkdownCommand::Subscript => wrapped_command_edit(text, selection, "~", "~", "subscript"),
        MarkdownCommand::Paragraph
        | MarkdownCommand::Heading(_)
        | MarkdownCommand::BulletList
        | MarkdownCommand::NumberedList
        | MarkdownCommand::Checklist
        | MarkdownCommand::Quote => unreachable!("block commands are handled above"),
    }
}

fn markdown_multi_command_edit(
    text: &str,
    selections: &[Range<usize>],
    command: MarkdownCommand,
) -> Option<MarkdownMultiCommandEdit> {
    let mut selections = if selections.is_empty() {
        vec![text.len()..text.len()]
    } else {
        selections
            .iter()
            .map(|selection| {
                let start = floor_char_boundary(text, selection.start.min(text.len()));
                let end = floor_char_boundary(text, selection.end.min(text.len()));
                start.min(end)..start.max(end)
            })
            .collect::<Vec<_>>()
    };
    selections.sort_by_key(|selection| (selection.start, selection.end));
    let mut normalized = Vec::<Range<usize>>::with_capacity(selections.len());
    for selection in selections {
        if let Some(previous) = normalized.last_mut()
            && (selection.start < previous.end
                || (selection.start == previous.start && selection.end == previous.end))
        {
            previous.end = previous.end.max(selection.end);
            continue;
        }
        normalized.push(selection);
    }

    let command_ranges = if block_command_prefix(command, 0).is_some() {
        let mut lines = normalized
            .into_iter()
            .map(|selection| line_range(text, selection))
            .collect::<Vec<_>>();
        lines.sort_by_key(|range| (range.start, range.end));
        let mut merged = Vec::<Range<usize>>::with_capacity(lines.len());
        for line in lines {
            if let Some(previous) = merged.last_mut()
                && line.start <= previous.end
            {
                previous.end = previous.end.max(line.end);
            } else {
                merged.push(line);
            }
        }
        merged
    } else {
        normalized
    };
    let mut edits = command_ranges
        .into_iter()
        .map(|selection| markdown_command_edit(text, selection, command))
        .collect::<Vec<_>>();
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
    if edits
        .windows(2)
        .any(|edits| edits[0].range.end > edits[1].range.start)
    {
        return None;
    }

    let mut updated = text.to_owned();
    for edit in edits.iter().rev() {
        updated.replace_range(edit.range.clone(), &edit.replacement);
    }
    let mut delta = 0isize;
    let selections = edits
        .iter()
        .map(|edit| {
            let selection = shift_range(&edit.selection, delta);
            delta += edit.replacement.len() as isize - edit.range.len() as isize;
            selection
        })
        .collect::<Vec<_>>();
    Some(MarkdownMultiCommandEdit {
        edits,
        text: updated,
        selections,
    })
}

fn shift_range(range: &Range<usize>, delta: isize) -> Range<usize> {
    range.start.saturating_add_signed(delta)..range.end.saturating_add_signed(delta)
}

fn floor_char_boundary(text: &str, mut offset: usize) -> usize {
    offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn next_char_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .chars()
        .next()
        .map(|character| offset + character.len_utf8())
        .unwrap_or(offset)
}

fn previous_char_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[..offset]
        .char_indices()
        .next_back()
        .map(|(offset, _)| offset)
        .unwrap_or(0)
}

pub fn document_stats(text: &str) -> DocumentStats {
    DocumentStats {
        words: text.split_whitespace().count(),
        characters: text.chars().count(),
        lines: if text.is_empty() {
            0
        } else {
            text.lines().count()
        },
    }
}

pub fn document_analysis(markdown: &str) -> DocumentAnalysis {
    document_analysis_with_source(markdown, None)
}

fn document_analysis_with_source(
    markdown: &str,
    lexical_source: Option<&Value>,
) -> DocumentAnalysis {
    DocumentAnalysis {
        stats: document_stats(markdown),
        outline: markdown_outline(markdown),
        blocks: markdown_block_summaries_with_source(markdown, lexical_source),
    }
}

pub fn markdown_outline(markdown: &str) -> Vec<OutlineItem> {
    let mut in_fence = false;
    markdown
        .lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let trimmed = raw.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                return None;
            }
            if in_fence {
                return None;
            }
            let hashes = trimmed
                .chars()
                .take_while(|character| *character == '#')
                .count();
            if !(1..=6).contains(&hashes) || !trimmed[hashes..].starts_with(' ') {
                return None;
            }
            let title = trimmed[hashes..].trim().trim_end_matches('#').trim();
            (!title.is_empty()).then(|| OutlineItem {
                level: hashes as u8,
                title: title.to_owned(),
                line: line as u32,
            })
        })
        .collect()
}

/// Converts Macro's serialized Lexical tree (the sync worker's `/raw`
/// response) into Markdown for the native Zed text surface. Unsupported nodes
/// retain their descendant text so a newer node never disappears visually;
/// the authoritative Loro document remains lossless.
pub fn lexical_state_to_markdown(state: &serde_json::Value) -> String {
    let root = state.get("root").unwrap_or(state);
    render_children(root, RenderContext::Block)
        .trim_end()
        .to_owned()
}

/// Serialize the native editor tree to Macro's internal Markdown dialect.
///
/// The visual projection intentionally renders atomic decorators as readable
/// tokens such as `@Nick`. Service payloads cannot use that lossy projection:
/// Macro's web client persists mentions as JSON-bearing `m-*` tags so another
/// client can reconstruct the exact entity. Replacing decorators in a clone
/// keeps the ordinary Markdown renderer authoritative for every other block
/// and inline format while retaining the wire representation of mentions.
pub fn lexical_state_to_macro_markdown(state: &serde_json::Value) -> String {
    let mut serialized = state.clone();
    replace_mentions_with_macro_markdown(&mut serialized);
    lexical_state_to_markdown(&serialized)
}

fn replace_mentions_with_macro_markdown(node: &mut Value) {
    if let Some(markdown) = macro_mention_markdown(node) {
        let metadata = node
            .get("$")
            .cloned()
            .unwrap_or_else(|| json!({"id": node_id()}));
        *node = json!({
            "$": metadata,
            "type": "text",
            "version": 1,
            "text": markdown,
            "format": 0,
            "detail": 0,
            "mode": "normal",
            "style": "",
        });
        return;
    }
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            replace_mentions_with_macro_markdown(child);
        }
    }
    if let Some(root) = node.get_mut("root") {
        replace_mentions_with_macro_markdown(root);
    }
}

fn macro_mention_markdown(node: &Value) -> Option<String> {
    let kind = node.get("type").and_then(Value::as_str)?;
    let (tag, fields): (&str, &[&str]) = match kind {
        "user-mention" => (
            "m-user-mention",
            &["userId", "email", "displayName", "mentionUuid"],
        ),
        "document-mention" => (
            "m-document-mention",
            &[
                "documentId",
                "documentName",
                "blockName",
                "blockParams",
                "collapsed",
                "mentionUuid",
                "channelType",
            ],
        ),
        "contact-mention" => (
            "m-contact-mention",
            &["contactId", "name", "emailOrDomain", "isCompany"],
        ),
        "group-mention" => ("m-group-mention", &["groupAlias"]),
        "date-mention" => ("m-date-mention", &["date", "displayFormat", "mentionUuid"]),
        "tag-mention" => (
            "m-tag",
            &["optionId", "propertyDefinitionId", "scope", "name", "color"],
        ),
        _ => return None,
    };
    let mut payload = Map::new();
    for field in fields {
        if let Some(value) = node.get(*field)
            && !value.is_null()
        {
            payload.insert((*field).to_owned(), value.clone());
        }
    }
    Some(format!(
        "<{tag}>{}</{tag}>",
        serde_json::to_string(&payload).expect("mention payload is JSON")
    ))
}

/// Resolve a Markdown byte offset to the stable Lexical text node and Unicode
/// offset used by Macro's collaboration awareness protocol. Markdown syntax
/// inserted by the projection (heading markers, list bullets, emphasis, links,
/// and table separators) is snapped to the nearest text boundary.
pub fn markdown_offset_to_lexical_point(
    state: &Value,
    markdown_offset: usize,
) -> Option<LexicalTextPoint> {
    let markdown = lexical_state_to_markdown(state);
    let offset = markdown_offset.min(markdown.len());
    let spans = lexical_text_spans(state);
    let span = spans
        .iter()
        .find(|span| (span.markdown_start..=span.markdown_end).contains(&offset))
        .or_else(|| {
            spans.iter().min_by_key(|span| {
                if offset < span.markdown_start {
                    span.markdown_start - offset
                } else {
                    offset.saturating_sub(span.markdown_end)
                }
            })
        })?;
    let bounded = offset.clamp(span.markdown_start, span.markdown_end);
    Some(LexicalTextPoint {
        node_id: span.node_id.clone(),
        unicode_offset: markdown[span.markdown_start..bounded].chars().count(),
    })
}

/// Resolve a remote Lexical/Loro text point back into the native Markdown
/// surface. This is the inverse of `markdown_offset_to_lexical_point` for
/// supported text nodes and clamps cursors that reference deleted text.
pub fn lexical_point_to_markdown_offset(state: &Value, point: &LexicalTextPoint) -> Option<usize> {
    let markdown = lexical_state_to_markdown(state);
    let span = lexical_text_spans(state)
        .into_iter()
        .find(|span| span.node_id == point.node_id)?;
    let text = &markdown[span.markdown_start..span.markdown_end];
    let relative = text
        .char_indices()
        .nth(point.unicode_offset)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len());
    Some(span.markdown_start + relative)
}

/// Inserts an exact Macro decorator node at a native Markdown caret while
/// retaining all existing Lexical IDs and metadata. The returned offset is the
/// visible caret position after the mention and its separating space.
pub fn insert_inline_mention(
    state: &Value,
    markdown_offset: usize,
    mention: &MacroInlineMention,
) -> Result<(Value, usize), MentionInsertionError> {
    let token = mention.visible_token();
    if token.len() <= 1 || token.contains(['\n', '\r']) {
        return Err(MentionInsertionError::MissingLabel);
    }
    let markdown = lexical_state_to_markdown(state);
    let offset = floor_char_boundary(&markdown, markdown_offset.min(markdown.len()));
    let mut updated = state.clone();
    let root = if updated.get("root").is_some() {
        updated.get_mut("root").unwrap()
    } else {
        &mut updated
    };
    let children = root
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .ok_or(MentionInsertionError::SelectionDoesNotResolveToText)?;

    if markdown.is_empty() {
        let paragraph = children
            .iter_mut()
            .find(|node| {
                node.get("type").and_then(Value::as_str) == Some("paragraph")
                    && node
                        .get("children")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
            })
            .ok_or(MentionInsertionError::SelectionDoesNotResolveToText)?;
        let paragraph_children = paragraph
            .get_mut("children")
            .and_then(Value::as_array_mut)
            .ok_or(MentionInsertionError::SelectionDoesNotResolveToText)?;
        paragraph_children.push(mention.serialized_node());
        return Ok((updated, token.len()));
    }

    let point = markdown_offset_to_lexical_point(state, offset)
        .ok_or(MentionInsertionError::SelectionDoesNotResolveToText)?;
    if lexical_point_to_markdown_offset(state, &point) != Some(offset) {
        return Err(MentionInsertionError::SelectionDoesNotResolveToText);
    }
    if !insert_mention_at_text_point(children, &point, mention.serialized_node()) {
        return Err(MentionInsertionError::SelectionDoesNotResolveToText);
    }
    let separator_len = if markdown[offset..].is_empty()
        || markdown[offset..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
    {
        0
    } else {
        1
    };
    Ok((updated, offset + token.len() + separator_len))
}

fn insert_mention_at_text_point(
    children: &mut Vec<Value>,
    point: &LexicalTextPoint,
    mention: Value,
) -> bool {
    for index in 0..children.len() {
        let stable_id = children[index].pointer("/$/id").and_then(Value::as_str);
        if stable_id == Some(point.node_id.as_str())
            && children[index].get("type").and_then(Value::as_str) == Some("text")
        {
            let mut original = children.remove(index);
            let text = original
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let split = char_to_byte_offset(&text, point.unicode_offset.min(text.chars().count()));
            let before = &text[..split];
            let after = &text[split..];
            let mut inserted = Vec::with_capacity(4);
            if !before.is_empty() {
                let mut before_node = original.clone();
                set_text_node_content(&mut before_node, before, None);
                inserted.push(before_node);
            }
            inserted.push(mention);
            if !after.is_empty() && !after.starts_with(char::is_whitespace) {
                inserted.push(text_node(" ", 0));
            }
            if !after.is_empty() {
                set_text_node_content(&mut original, after, (!before.is_empty()).then(node_id));
                inserted.push(original);
            }
            children.splice(index..index, inserted);
            return true;
        }
        if let Some(grandchildren) = children[index]
            .get_mut("children")
            .and_then(Value::as_array_mut)
            && insert_mention_at_text_point(grandchildren, point, mention.clone())
        {
            return true;
        }
    }
    false
}

/// Wrap the selected native Markdown text in Macro's serialized
/// `comment-mark` nodes. Multi-node selections create multiple wrappers with
/// the same mark ID, matching Lexical MarkNode behavior, while the visible
/// Markdown projection remains unchanged.
pub fn anchor_document_comment(
    state: &Value,
    selection: Range<usize>,
    mark_id: &str,
    thread_id: i64,
) -> Result<Value, CommentAnchorError> {
    anchor_document_comment_with_status(state, selection, mark_id, thread_id, false)
}

/// Creates the same local-only draft mark as Macro's Lexical comment plugin.
/// Draft marks use the wire-compatible sentinel thread ID (`-1`) and are not
/// durable until the comment API has created the server thread.
pub fn anchor_document_comment_draft(
    state: &Value,
    selection: Range<usize>,
    mark_id: &str,
) -> Result<Value, CommentAnchorError> {
    anchor_document_comment_with_status(state, selection, mark_id, -1, true)
}

fn anchor_document_comment_with_status(
    state: &Value,
    selection: Range<usize>,
    mark_id: &str,
    thread_id: i64,
    is_draft: bool,
) -> Result<Value, CommentAnchorError> {
    if selection.start >= selection.end {
        return Err(CommentAnchorError::EmptySelection);
    }
    let markdown = lexical_state_to_markdown(state);
    let selection = floor_char_boundary(&markdown, selection.start.min(markdown.len()))
        ..floor_char_boundary(&markdown, selection.end.min(markdown.len()));
    if selection.start >= selection.end {
        return Err(CommentAnchorError::EmptySelection);
    }
    let mut node_ranges = HashMap::<String, Range<usize>>::new();
    for span in lexical_text_spans(state) {
        let start = selection.start.max(span.markdown_start);
        let end = selection.end.min(span.markdown_end);
        if start >= end {
            continue;
        }
        node_ranges.insert(
            span.node_id,
            markdown[span.markdown_start..start].chars().count()
                ..markdown[span.markdown_start..end].chars().count(),
        );
    }
    if node_ranges.is_empty() {
        return Err(CommentAnchorError::SelectionDoesNotResolveToText);
    }
    let mut annotated = state.clone();
    let root = if annotated.get("root").is_some() {
        annotated.get_mut("root").unwrap()
    } else {
        &mut annotated
    };
    let children = root
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .ok_or(CommentAnchorError::SelectionDoesNotResolveToText)?;
    wrap_comment_ranges(children, &node_ranges, mark_id, thread_id, is_draft, false)?;
    Ok(annotated)
}

/// Converts every fragment of a local draft mark into a durable server-backed
/// discussion without re-wrapping the text or changing any node identity.
/// Returns `None` when the mark is no longer present (for example after a
/// concurrent edit deleted the selected text).
pub fn finalize_document_comment_mark(
    state: &Value,
    mark_id: &str,
    thread_id: i64,
) -> Option<Value> {
    let mut state = state.clone();
    let root = if state.get("root").is_some() {
        state.get_mut("root")
    } else {
        Some(&mut state)
    };
    let children = root
        .and_then(|root| root.get_mut("children"))
        .and_then(Value::as_array_mut)?;
    finalize_comment_mark_in_children(children, mark_id, thread_id).then_some(state)
}

fn finalize_comment_mark_in_children(
    children: &mut [Value],
    mark_id: &str,
    thread_id: i64,
) -> bool {
    let mut found = false;
    for child in children {
        let carries_mark = child.get("type").and_then(Value::as_str) == Some("comment-mark")
            && child
                .get("ids")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(mark_id)));
        if carries_mark {
            if let Some(object) = child.as_object_mut() {
                object.insert("threadId".into(), thread_id.into());
                object.insert("isDraft".into(), false.into());
            }
            found = true;
        }
        if let Some(grandchildren) = child.get_mut("children").and_then(Value::as_array_mut) {
            found |= finalize_comment_mark_in_children(grandchildren, mark_id, thread_id);
        }
    }
    found
}

/// Remove one Macro discussion mark without changing the visible document.
/// A Lexical mark can carry more than one discussion ID; in that case only the
/// requested ID is removed and the wrapper remains. Empty wrappers are
/// unwrapped so their children retain their exact formatting and identities.
pub fn remove_document_comment_mark(state: &Value, mark_id: &str) -> Value {
    let mut state = state.clone();
    let root = if state.get("root").is_some() {
        state.get_mut("root")
    } else {
        Some(&mut state)
    };
    if let Some(children) = root
        .and_then(|root| root.get_mut("children"))
        .and_then(Value::as_array_mut)
    {
        remove_comment_mark_from_children(children, mark_id);
    }
    state
}

fn remove_comment_mark_from_children(children: &mut Vec<Value>, mark_id: &str) {
    let mut next = Vec::with_capacity(children.len());
    for mut child in std::mem::take(children) {
        if let Some(grandchildren) = child.get_mut("children").and_then(Value::as_array_mut) {
            remove_comment_mark_from_children(grandchildren, mark_id);
        }
        let is_comment_mark = child.get("type").and_then(Value::as_str) == Some("comment-mark");
        let carries_mark = child
            .get("ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(mark_id)));
        if !is_comment_mark || !carries_mark {
            next.push(child);
            continue;
        }

        let remaining_ids = child
            .get("ids")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|id| id.as_str() != Some(mark_id))
            .cloned()
            .collect::<Vec<_>>();
        if remaining_ids.is_empty() {
            next.extend(
                child
                    .get_mut("children")
                    .and_then(Value::as_array_mut)
                    .map(std::mem::take)
                    .unwrap_or_default(),
            );
        } else {
            if let Some(object) = child.as_object_mut() {
                object.insert("ids".into(), Value::Array(remaining_ids));
            }
            next.push(child);
        }
    }
    *children = next;
}

fn wrap_comment_ranges(
    children: &mut Vec<Value>,
    node_ranges: &HashMap<String, Range<usize>>,
    mark_id: &str,
    thread_id: i64,
    is_draft: bool,
    inside_comment: bool,
) -> Result<(), CommentAnchorError> {
    let mut next = Vec::with_capacity(children.len());
    for mut child in std::mem::take(children) {
        let kind = child
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let child_inside_comment = inside_comment || kind == "comment-mark";
        let stable_id = child
            .pointer("/$/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(range) = stable_id.as_ref().and_then(|id| node_ranges.get(id)) {
            if child_inside_comment {
                return Err(CommentAnchorError::NestedComment);
            }
            let text = child
                .get("text")
                .and_then(Value::as_str)
                .ok_or(CommentAnchorError::SelectionDoesNotResolveToText)?
                .to_owned();
            let char_count = text.chars().count();
            let start = range.start.min(char_count);
            let end = range.end.min(char_count);
            if start >= end {
                next.push(child);
                continue;
            }
            let start_byte = char_to_byte_offset(&text, start);
            let end_byte = char_to_byte_offset(&text, end);
            let before = &text[..start_byte];
            let selected = &text[start_byte..end_byte];
            let after = &text[end_byte..];
            if !before.is_empty() {
                let mut before_node = child.clone();
                set_text_node_content(&mut before_node, before, None);
                next.push(before_node);
            }
            let mut selected_node = child.clone();
            set_text_node_content(
                &mut selected_node,
                selected,
                (!before.is_empty()).then(node_id),
            );
            next.push(comment_mark_node(
                mark_id,
                thread_id,
                is_draft,
                selected_node,
            ));
            if !after.is_empty() {
                set_text_node_content(&mut child, after, Some(node_id()));
                next.push(child);
            }
        } else {
            if let Some(grandchildren) = child.get_mut("children").and_then(Value::as_array_mut) {
                wrap_comment_ranges(
                    grandchildren,
                    node_ranges,
                    mark_id,
                    thread_id,
                    is_draft,
                    child_inside_comment,
                )?;
            }
            next.push(child);
        }
    }
    *children = next;
    Ok(())
}

fn comment_mark_node(mark_id: &str, thread_id: i64, is_draft: bool, child: Value) -> Value {
    json!({
        "$": {"id": node_id()},
        "children": [child],
        "direction": null,
        "format": "",
        "ids": [mark_id],
        "indent": 0,
        "isDraft": is_draft,
        "threadId": thread_id,
        "type": "comment-mark",
        "version": 1
    })
}

fn set_text_node_content(node: &mut Value, text: &str, replacement_id: Option<String>) {
    if let Some(node) = node.as_object_mut() {
        node.insert("text".into(), text.into());
        if let Some(replacement_id) = replacement_id
            && let Some(metadata) = node.get_mut("$").and_then(Value::as_object_mut)
        {
            metadata.insert("id".into(), replacement_id.into());
        }
    }
}

fn char_to_byte_offset(text: &str, offset: usize) -> usize {
    text.char_indices()
        .nth(offset)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

fn lexical_text_spans(state: &Value) -> Vec<LexicalTextSpan> {
    let projected_len = lexical_state_to_markdown(state).len();
    let root = state.get("root").unwrap_or(state);
    let Some(children) = root.get("children").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut spans = Vec::new();
    let mut document_offset = 0;
    let mut has_rendered_block = false;
    for child in children {
        let rendered = render_node(child, RenderContext::Block);
        if rendered.is_empty() {
            continue;
        }
        if has_rendered_block {
            document_offset += 2;
        }
        has_rendered_block = true;
        let mut text_nodes = Vec::new();
        collect_text_nodes(child, &mut text_nodes);
        let mut search_offset = 0;
        for (node_id, text) in text_nodes {
            if text.is_empty() {
                spans.push(LexicalTextSpan {
                    node_id,
                    markdown_start: document_offset + search_offset,
                    markdown_end: document_offset + search_offset,
                });
                continue;
            }
            let Some(relative) = rendered[search_offset..].find(&text) else {
                continue;
            };
            let start = search_offset + relative;
            let end = start + text.len();
            spans.push(LexicalTextSpan {
                node_id,
                markdown_start: document_offset + start,
                markdown_end: document_offset + end,
            });
            search_offset = end;
        }
        document_offset += rendered.len();
    }
    for span in &mut spans {
        span.markdown_start = span.markdown_start.min(projected_len);
        span.markdown_end = span.markdown_end.min(projected_len);
    }
    spans
}

fn collect_text_nodes(node: &Value, nodes: &mut Vec<(String, String)>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if matches!(
        kind,
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight"
    ) && let (Some(node_id), Some(text)) = (
        node.get("$")
            .and_then(|metadata| metadata.get("id"))
            .and_then(Value::as_str),
        node.get("text").and_then(Value::as_str),
    ) {
        nodes.push((node_id.to_owned(), text.to_owned()));
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_text_nodes(child, nodes);
        }
    }
}

/// Convert native Markdown edits to Macro's serialized Lexical node schema.
/// The returned value is consumed by `macro_sync`, which stores the recursive
/// `$`/`text`/`children` fields in their corresponding Loro containers.
pub fn markdown_to_lexical_state(markdown: &str) -> Value {
    let (markdown, protected_mentions) = protect_macro_inline_mentions(markdown);
    let mut stack = Vec::<LexicalFrame>::new();
    let mut roots = Vec::<Value>::new();
    let mut format = 0_u32;

    for event in Parser::new_ext(&markdown, document_markdown_options()) {
        match event {
            Event::Start(tag) => match tag {
                Tag::Emphasis => format |= 2,
                Tag::Strong => format |= 1,
                Tag::Strikethrough => format |= 4,
                Tag::Superscript => format |= 64,
                Tag::Subscript => format |= 32,
                Tag::Paragraph => stack.push(LexicalFrame::new(FrameKind::Paragraph)),
                Tag::Heading { level, .. } => stack.push(LexicalFrame::new(FrameKind::Heading {
                    tag: level.to_string(),
                })),
                Tag::BlockQuote(_) => stack.push(LexicalFrame::new(FrameKind::Quote)),
                Tag::CodeBlock(kind) => stack.push(LexicalFrame::new(FrameKind::Code {
                    language: match kind {
                        CodeBlockKind::Indented => String::new(),
                        CodeBlockKind::Fenced(language) => language.into_string(),
                    },
                })),
                Tag::HtmlBlock => stack.push(LexicalFrame::new(FrameKind::Code {
                    language: "html".into(),
                })),
                Tag::List(start) => stack.push(LexicalFrame::new(FrameKind::List { start })),
                Tag::Item => stack.push(LexicalFrame::new(FrameKind::Item { checked: None })),
                Tag::Link {
                    dest_url, title, ..
                } => stack.push(LexicalFrame::new(FrameKind::Link {
                    url: dest_url.into_string(),
                    title: title.into_string(),
                })),
                Tag::Image { dest_url, .. } => stack.push(LexicalFrame::new(FrameKind::Image {
                    src: dest_url.into_string(),
                })),
                Tag::Table(_) => stack.push(LexicalFrame::new(FrameKind::Table)),
                Tag::TableHead => stack.push(LexicalFrame::new(FrameKind::TableHead)),
                Tag::TableRow => stack.push(LexicalFrame::new(FrameKind::TableRow)),
                Tag::TableCell => stack.push(LexicalFrame::new(FrameKind::TableCell)),
                // Lexical does not have a dedicated front-matter node. Store
                // the delimiter kind in the code node's existing language
                // field so Macro's Solid client retains it during
                // import/export instead of relying on an unknown JSON field.
                Tag::MetadataBlock(kind) => stack.push(LexicalFrame::new(FrameKind::Code {
                    language: match kind {
                        MetadataBlockKind::YamlStyle => "macro-frontmatter-yaml".into(),
                        MetadataBlockKind::PlusesStyle => "macro-frontmatter-toml".into(),
                    },
                })),
                Tag::FootnoteDefinition(label) => {
                    let mut frame = LexicalFrame::new(FrameKind::Paragraph);
                    frame.push(text_node(&format!("[^{label}]: "), 0));
                    stack.push(frame);
                }
                Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {
                    stack.push(LexicalFrame::new(FrameKind::Transparent))
                }
            },
            Event::End(end) => match end {
                TagEnd::Emphasis => format &= !2,
                TagEnd::Strong => format &= !1,
                TagEnd::Strikethrough => format &= !4,
                TagEnd::Superscript => format &= !64,
                TagEnd::Subscript => format &= !32,
                _ => finish_frame(&mut stack, &mut roots),
            },
            Event::Text(text) => push_node(&mut stack, &mut roots, text_node(&text, format)),
            Event::Code(text) => push_node(&mut stack, &mut roots, text_node(&text, format | 16)),
            Event::InlineMath(equation) => push_node(
                &mut stack,
                &mut roots,
                decorator_node(
                    "equation",
                    [
                        ("equation", json!(equation.into_string())),
                        ("inline", json!(true)),
                    ],
                ),
            ),
            Event::DisplayMath(equation) => push_node(
                &mut stack,
                &mut roots,
                decorator_node(
                    "equation",
                    [
                        ("equation", json!(equation.into_string())),
                        ("inline", json!(false)),
                    ],
                ),
            ),
            Event::Html(html) | Event::InlineHtml(html) => {
                push_node(&mut stack, &mut roots, text_node(&html, format))
            }
            Event::FootnoteReference(label) => push_node(
                &mut stack,
                &mut roots,
                text_node(&format!("[^{label}]"), format),
            ),
            Event::SoftBreak => push_node(&mut stack, &mut roots, text_node("\n", format)),
            Event::HardBreak => push_node(
                &mut stack,
                &mut roots,
                decorator_node("linebreak", std::iter::empty::<(&str, Value)>()),
            ),
            Event::Rule => push_node(
                &mut stack,
                &mut roots,
                decorator_node("horizontalrule", std::iter::empty::<(&str, Value)>()),
            ),
            Event::TaskListMarker(checked) => {
                if let Some(frame) = stack
                    .iter_mut()
                    .rev()
                    .find(|frame| matches!(frame.kind, FrameKind::Item { .. }))
                {
                    frame.kind = FrameKind::Item {
                        checked: Some(checked),
                    };
                }
            }
        }
    }

    while !stack.is_empty() {
        finish_frame(&mut stack, &mut roots);
    }
    if roots.is_empty() {
        roots.push(element_node("paragraph", Map::new(), Vec::new()));
    }
    let mut state = json!({
        "root": {
            "$": { "id": "root" },
            "children": roots,
            "direction": null,
            "format": "",
            "indent": 0,
            "type": "root",
            "version": 1
        }
    });
    restore_macro_inline_mentions(&mut state, &protected_mentions);
    state
}

const MAX_MACRO_MENTION_PAYLOAD_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct ProtectedMacroMention {
    marker: String,
    node: Value,
}

/// Protect Macro's JSON-bearing internal mention tags from the CommonMark HTML
/// parser, then restore them as typed Lexical decorators. The web client uses
/// the same six `m-*` tags for draft bodies and channel/document Markdown.
/// Invalid payloads remain literal text so untrusted markup cannot silently
/// acquire entity semantics.
fn protect_macro_inline_mentions(markdown: &str) -> (String, Vec<ProtectedMacroMention>) {
    const TAGS: [(&str, &str, &str); 6] = [
        ("m-user-mention", "<m-user-mention>", "</m-user-mention>"),
        (
            "m-document-mention",
            "<m-document-mention>",
            "</m-document-mention>",
        ),
        (
            "m-contact-mention",
            "<m-contact-mention>",
            "</m-contact-mention>",
        ),
        ("m-group-mention", "<m-group-mention>", "</m-group-mention>"),
        ("m-date-mention", "<m-date-mention>", "</m-date-mention>"),
        ("m-tag", "<m-tag>", "</m-tag>"),
    ];

    if !TAGS
        .iter()
        .any(|(_, opening, _)| markdown.contains(opening))
    {
        return (markdown.to_owned(), Vec::new());
    }

    let code_ranges = markdown_code_ranges(markdown);
    let mut protected = String::with_capacity(markdown.len());
    let mut mentions = Vec::new();
    let mut cursor = 0;

    while cursor < markdown.len() {
        let next = TAGS
            .iter()
            .filter_map(|(tag, opening, closing)| {
                markdown[cursor..]
                    .find(opening)
                    .map(|relative| (cursor + relative, *tag, *opening, *closing))
            })
            .min_by_key(|(start, _, _, _)| *start);
        let Some((start, tag, opening, closing)) = next else {
            protected.push_str(&markdown[cursor..]);
            break;
        };
        protected.push_str(&markdown[cursor..start]);
        let payload_start = start + opening.len();
        let Some(relative_end) = markdown[payload_start..].find(closing) else {
            protected.push_str(&markdown[start..]);
            break;
        };
        let payload_end = payload_start + relative_end;
        let end = payload_end + closing.len();
        let overlaps_code = code_ranges
            .iter()
            .any(|range| range.start < end && start < range.end);
        let payload = &markdown[payload_start..payload_end];
        if !overlaps_code
            && payload.len() <= MAX_MACRO_MENTION_PAYLOAD_BYTES
            && let Some(node) = macro_mention_node_from_payload(tag, payload)
        {
            let marker = format!(
                "\u{e000}macro-mention-{}-{}\u{e001}",
                mentions.len(),
                Uuid::new_v4()
            );
            protected.push_str(&marker);
            mentions.push(ProtectedMacroMention { marker, node });
        } else {
            protected.push_str(&markdown[start..end]);
        }
        cursor = end;
    }

    (protected, mentions)
}

fn markdown_code_ranges(markdown: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut code_block_depth = 0_u32;
    for (event, range) in Parser::new_ext(markdown, document_markdown_options()).into_offset_iter()
    {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                code_block_depth += 1;
                ranges.push(range);
            }
            Event::End(TagEnd::CodeBlock) => {
                ranges.push(range);
                code_block_depth = code_block_depth.saturating_sub(1);
            }
            Event::Code(_) => ranges.push(range),
            _ if code_block_depth > 0 => ranges.push(range),
            _ => {}
        }
    }
    ranges
}

fn macro_mention_node_from_payload(tag: &str, payload: &str) -> Option<Value> {
    let payload = serde_json::from_str::<Value>(payload).ok()?;
    let payload = payload.as_object()?;
    let required_string = |field: &str| {
        payload
            .get(field)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut node = match tag {
        "m-user-mention" => json!({
            "$": {"id": node_id()},
            "type": "user-mention",
            "version": 1,
            "userId": required_string("userId")?,
            "email": required_string("email")?,
        }),
        "m-document-mention" => json!({
            "$": {"id": node_id()},
            "type": "document-mention",
            "version": 2,
            "documentId": required_string("documentId")?,
            "documentName": required_string("documentName")?,
            "blockName": payload.get("blockName").and_then(Value::as_str).unwrap_or("unknown"),
            "blockParams": payload.get("blockParams").cloned().unwrap_or_else(|| json!({})),
            "collapsed": payload.get("collapsed").and_then(Value::as_bool).unwrap_or(false),
        }),
        "m-contact-mention" => json!({
            "$": {"id": node_id()},
            "type": "contact-mention",
            "version": 1,
            "contactId": required_string("contactId")?,
            "name": required_string("name")?,
            "emailOrDomain": required_string("emailOrDomain")?,
            "isCompany": payload.get("isCompany").and_then(Value::as_bool)?,
        }),
        "m-group-mention" => json!({
            "$": {"id": node_id()},
            "type": "group-mention",
            "version": 1,
            "groupAlias": required_string("groupAlias")?,
        }),
        "m-date-mention" => json!({
            "$": {"id": node_id()},
            "type": "date-mention",
            "version": 1,
            "date": required_string("date")?,
            "displayFormat": required_string("displayFormat")?,
        }),
        "m-tag" => {
            let scope = required_string("scope")?;
            if !matches!(scope.as_str(), "user" | "team") {
                return None;
            }
            json!({
                "$": {"id": node_id()},
                "type": "tag-mention",
                "version": 1,
                "optionId": required_string("optionId")?,
                "propertyDefinitionId": required_string("propertyDefinitionId")?,
                "scope": scope,
                "name": required_string("name")?,
            })
        }
        _ => return None,
    };

    let optional_fields: &[(&str, fn(&Value) -> bool)] = match tag {
        "m-user-mention" => &[
            ("displayName", Value::is_string),
            ("mentionUuid", Value::is_string),
        ],
        "m-document-mention" => &[
            ("mentionUuid", Value::is_string),
            ("channelType", Value::is_string),
        ],
        "m-contact-mention" => &[("mentionUuid", Value::is_string)],
        "m-date-mention" => &[("mentionUuid", Value::is_string)],
        "m-tag" => &[("color", Value::is_string)],
        _ => &[],
    };
    for (field, validate) in optional_fields {
        if let Some(value) = payload.get(*field) {
            if value.is_null() {
                continue;
            }
            if !validate(value) {
                return None;
            }
            node.as_object_mut()?
                .insert((*field).to_owned(), value.clone());
        }
    }
    if tag == "m-document-mention"
        && (payload
            .get("blockParams")
            .is_some_and(|value| !value.is_object())
            || payload
                .get("collapsed")
                .is_some_and(|value| !value.is_boolean()))
    {
        return None;
    }
    Some(node)
}

fn restore_macro_inline_mentions(state: &mut Value, mentions: &[ProtectedMacroMention]) {
    if mentions.is_empty() {
        return;
    }
    if let Some(children) = state.get_mut("children").and_then(Value::as_array_mut) {
        let mut restored = Vec::with_capacity(children.len());
        for mut child in std::mem::take(children) {
            restore_macro_inline_mentions(&mut child, mentions);
            if child.get("type").and_then(Value::as_str) == Some("text") {
                restored.extend(expand_protected_text_node(child, mentions));
            } else {
                restored.push(child);
            }
        }
        *children = restored;
    }
    if let Some(root) = state.get_mut("root") {
        restore_macro_inline_mentions(root, mentions);
    }
}

fn expand_protected_text_node(
    mut original: Value,
    mentions: &[ProtectedMacroMention],
) -> Vec<Value> {
    let text = original
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if !mentions
        .iter()
        .any(|mention| text.contains(&mention.marker))
    {
        return vec![original];
    }

    let mut expanded = Vec::new();
    let mut cursor = 0;
    let mut used_original_id = false;
    while cursor < text.len() {
        let next = mentions
            .iter()
            .filter_map(|mention| {
                text[cursor..]
                    .find(&mention.marker)
                    .map(|relative| (cursor + relative, mention))
            })
            .min_by_key(|(start, _)| *start);
        let Some((start, mention)) = next else {
            push_restored_text_fragment(
                &mut expanded,
                &mut original,
                &text[cursor..],
                &mut used_original_id,
            );
            break;
        };
        push_restored_text_fragment(
            &mut expanded,
            &mut original,
            &text[cursor..start],
            &mut used_original_id,
        );
        expanded.push(mention.node.clone());
        cursor = start + mention.marker.len();
    }
    expanded
}

fn push_restored_text_fragment(
    expanded: &mut Vec<Value>,
    original: &mut Value,
    text: &str,
    used_original_id: &mut bool,
) {
    if text.is_empty() {
        return;
    }
    let mut fragment = original.clone();
    let replacement_id = (*used_original_id).then(node_id);
    set_text_node_content(&mut fragment, text, replacement_id);
    *used_original_id = true;
    expanded.push(fragment);
}

/// Projects edits from the native Markdown surface back onto an authoritative
/// Macro Lexical state. Only top-level blocks understood by the native editor
/// are returned because `macro_sync` reconciles those slots while retaining
/// opaque top-level containers in place. Root metadata, node IDs, and fields
/// introduced by newer Macro clients are copied forward.
///
/// Blocks with unsupported descendants are treated as opaque. They may coexist
/// with edits elsewhere in the document, but a change to their rendered value
/// is rejected so embeds and future nodes can never be silently flattened into
/// text. Macro mention decorators are a deliberate exception: their complete
/// serialized nodes are re-woven when their visible token survives unchanged.
pub fn project_markdown_edits(
    source_state: &Value,
    markdown: &str,
) -> Result<Value, LexicalProjectionError> {
    let parsed = markdown_to_lexical_state(markdown);
    let parsed_children = parsed
        .get("root")
        .and_then(|root| root.get("children"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let source_root = source_state.get("root").unwrap_or(source_state);
    let source_children = source_root
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let opaque = source_children
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            let kind = node
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let unsupported = first_unsupported_descendant(node);
            (!is_native_editable_top_level(kind) || unsupported.is_some()).then(|| OpaqueBlock {
                source_index: index,
                node,
                node_type: unsupported.unwrap_or(kind),
                markdown: normalize_projected_block(&render_node(node, RenderContext::Block))
                    .to_owned(),
            })
        })
        .collect::<Vec<_>>();
    let parsed_markdown = parsed_children
        .iter()
        .map(|node| normalize_projected_block(&render_node(node, RenderContext::Block)).to_owned())
        .collect::<Vec<_>>();
    let spans = match_unique_opaque_blocks(&opaque, &parsed_markdown).map_err(|anchor| {
        LexicalProjectionError {
            block_index: anchor.source_index,
            node_type: anchor.node_type.to_owned(),
        }
    })?;

    let mut projected = Vec::with_capacity(parsed_children.len().max(source_children.len()));
    let (mut source_cursor, mut parsed_cursor) = (0, 0);
    for (anchor, (parsed_start, parsed_end)) in opaque.iter().zip(spans) {
        project_editable_segment(
            &source_children[source_cursor..anchor.source_index],
            &parsed_children[parsed_cursor..parsed_start],
            &mut projected,
            source_cursor,
        )?;
        projected.push(anchor.node.clone());
        source_cursor = anchor.source_index + 1;
        parsed_cursor = parsed_end;
    }
    project_editable_segment(
        &source_children[source_cursor..],
        &parsed_children[parsed_cursor..],
        &mut projected,
        source_cursor,
    )?;

    let mut root = source_root
        .as_object()
        .cloned()
        .unwrap_or_else(|| parsed["root"].as_object().cloned().unwrap_or_default());
    root.insert("children".into(), Value::Array(projected));
    Ok(json!({ "root": Value::Object(root) }))
}

struct OpaqueBlock<'a> {
    source_index: usize,
    node: &'a Value,
    node_type: &'a str,
    markdown: String,
}

/// Finds one unambiguous ordered occurrence for every opaque block in the
/// native Markdown projection. Exact source nodes replace those occurrences;
/// removing or ambiguously moving one is rejected rather than silently
/// flattening comments, mentions, embeds, or future schema data.
fn match_unique_opaque_blocks<'a>(
    opaque: &'a [OpaqueBlock<'a>],
    parsed: &[String],
) -> Result<Vec<(usize, usize)>, &'a OpaqueBlock<'a>> {
    let candidates = opaque
        .iter()
        .map(|anchor| opaque_candidates(&anchor.markdown, parsed))
        .collect::<Vec<_>>();
    if let Some((index, _)) = candidates
        .iter()
        .enumerate()
        .find(|(_, candidates)| candidates.is_empty())
    {
        return Err(&opaque[index]);
    }

    let mut solutions = Vec::new();
    collect_opaque_matches(&candidates, 0, 0, &mut Vec::new(), &mut solutions);
    match solutions.as_slice() {
        [solution] => Ok(solution.clone()),
        [] => Err(&opaque[0]),
        _ => {
            let ambiguous = candidates
                .iter()
                .position(|candidates| candidates.len() > 1)
                .unwrap_or(0);
            Err(&opaque[ambiguous])
        }
    }
}

fn opaque_candidates(target: &str, parsed: &[String]) -> Vec<(usize, usize)> {
    if target.is_empty() {
        // Invisible metadata/decorator nodes consume no Markdown. Keeping a
        // single zero-width anchor preserves them without making every cursor
        // boundary an ambiguous match.
        return vec![(usize::MAX, usize::MAX)];
    }
    let mut candidates = Vec::new();
    for start in 0..parsed.len() {
        let mut joined = String::new();
        for (end, block) in parsed.iter().enumerate().skip(start) {
            if !joined.is_empty() {
                joined.push_str("\n\n");
            }
            joined.push_str(block);
            let normalized = normalize_projected_block(&joined);
            if normalized == target {
                candidates.push((start, end + 1));
            }
            if normalized.len() > target.len() {
                break;
            }
        }
    }
    candidates
}

fn collect_opaque_matches(
    candidates: &[Vec<(usize, usize)>],
    anchor_index: usize,
    parsed_cursor: usize,
    current: &mut Vec<(usize, usize)>,
    solutions: &mut Vec<Vec<(usize, usize)>>,
) {
    if solutions.len() > 1 {
        return;
    }
    if anchor_index == candidates.len() {
        solutions.push(current.clone());
        return;
    }
    for &(start, end) in &candidates[anchor_index] {
        if start == usize::MAX {
            current.push((parsed_cursor, parsed_cursor));
            collect_opaque_matches(
                candidates,
                anchor_index + 1,
                parsed_cursor,
                current,
                solutions,
            );
            current.pop();
            continue;
        }
        if start < parsed_cursor {
            continue;
        }
        current.push((start, end));
        collect_opaque_matches(candidates, anchor_index + 1, end, current, solutions);
        current.pop();
        if solutions.len() > 1 {
            return;
        }
    }
}

fn project_editable_segment(
    source: &[Value],
    parsed: &[Value],
    projected: &mut Vec<Value>,
    source_offset: usize,
) -> Result<(), LexicalProjectionError> {
    for (index, incoming) in parsed.iter().enumerate() {
        let merged = match source.get(index) {
            Some(existing) => {
                let restored = if contains_preserved_inline_node(existing) {
                    restore_preserved_inline_nodes(existing, incoming).map_err(|node_type| {
                        LexicalProjectionError {
                            block_index: source_offset + index,
                            node_type,
                        }
                    })?
                } else {
                    incoming.clone()
                };
                if contains_document_comment_marks(existing) {
                    reapply_document_comment_marks(existing, &restored).map_err(|()| {
                        LexicalProjectionError {
                            block_index: source_offset + index,
                            node_type: "comment-mark".into(),
                        }
                    })?
                } else {
                    merge_editable_node(existing, &restored)
                }
            }
            None => incoming.clone(),
        };
        projected.push(merged);
    }
    Ok(())
}

#[derive(Clone)]
struct PreservedInlineTemplate {
    node: Value,
    node_type: String,
    plain_range: Range<usize>,
    rendered: String,
}

fn is_preserved_inline_node(kind: &str) -> bool {
    kind == "mention" || kind.ends_with("-mention") || matches!(kind, "snapshot" | "watermark")
}

fn contains_preserved_inline_node(node: &Value) -> bool {
    node.get("type")
        .and_then(Value::as_str)
        .is_some_and(is_preserved_inline_node)
        || node
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(contains_preserved_inline_node))
}

/// Restore Macro's atomic inline decorators after the Markdown parser has
/// represented their visible labels as ordinary text. A decorator may move as
/// surrounding text changes, but its rendered token must remain equivalent and
/// the original serialized node is always inserted intact.
fn restore_preserved_inline_nodes(existing: &Value, incoming: &Value) -> Result<Value, String> {
    let mut old_cursor = 0;
    let mut templates = Vec::new();
    collect_preserved_inline_templates(existing, &mut old_cursor, &mut templates);
    if templates.is_empty() {
        return Ok(incoming.clone());
    }

    let old_plain = lexical_projection_text(existing);
    let new_plain = lexical_projection_text(incoming);
    let mut mapped = templates
        .into_iter()
        .map(|template| {
            let node_type = template.node_type.clone();
            let plain_range =
                map_plain_range_through_edits(&old_plain, &new_plain, template.plain_range.clone())
                    .ok_or_else(|| node_type.clone())?;
            let visible =
                char_slice(&new_plain, plain_range.clone()).ok_or_else(|| node_type.clone())?;
            if visible != template.rendered {
                return Err(node_type);
            }
            Ok((template, plain_range))
        })
        .collect::<Result<Vec<_>, String>>()?;
    mapped.sort_by_key(|(_, range)| (range.start, range.end));
    if let Some(node_type) = mapped
        .windows(2)
        .find_map(|pair| (pair[0].1.end > pair[1].1.start).then(|| pair[1].0.node_type.clone()))
    {
        return Err(node_type);
    }

    let mut restored = incoming.clone();
    let mut plain_cursor = 0;
    let mut template_cursor = 0;
    replace_preserved_inline_ranges(
        &mut restored,
        &mapped,
        &mut plain_cursor,
        &mut template_cursor,
    )?;
    if template_cursor != mapped.len() {
        return Err(mapped[template_cursor].0.node_type.clone());
    }
    Ok(restored)
}

fn collect_preserved_inline_templates(
    node: &Value,
    plain_cursor: &mut usize,
    templates: &mut Vec<PreservedInlineTemplate>,
) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if is_preserved_inline_node(kind) {
        let rendered = render_node(node, RenderContext::Inline);
        let start = *plain_cursor;
        *plain_cursor += rendered.chars().count();
        templates.push(PreservedInlineTemplate {
            node: node.clone(),
            node_type: kind.to_owned(),
            plain_range: start..*plain_cursor,
            rendered,
        });
        return;
    }
    if is_textual_lexical_node(kind) {
        *plain_cursor += node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .count();
        return;
    }
    if kind == "linebreak" {
        *plain_cursor += 1;
        return;
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_preserved_inline_templates(child, plain_cursor, templates);
        }
    }
}

fn lexical_projection_text(node: &Value) -> String {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if is_preserved_inline_node(kind) {
        return render_node(node, RenderContext::Inline);
    }
    if is_textual_lexical_node(kind) {
        return node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
    }
    if kind == "linebreak" {
        return "\n".into();
    }
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(lexical_projection_text)
        .collect()
}

fn is_textual_lexical_node(kind: &str) -> bool {
    matches!(
        kind,
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight"
    )
}

fn replace_preserved_inline_ranges(
    node: &mut Value,
    templates: &[(PreservedInlineTemplate, Range<usize>)],
    plain_cursor: &mut usize,
    template_cursor: &mut usize,
) -> Result<(), String> {
    let kind = node
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if kind == "linebreak" {
        *plain_cursor += 1;
        return Ok(());
    }
    if is_textual_lexical_node(&kind) {
        *plain_cursor += node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .count();
        return Ok(());
    }
    let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) else {
        return Ok(());
    };

    let mut next = Vec::with_capacity(children.len() + templates.len());
    for mut child in std::mem::take(children) {
        let child_kind = child
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_textual_lexical_node(child_kind) {
            let text = child
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let node_start = *plain_cursor;
            let char_count = text.chars().count();
            let node_end = node_start + char_count;
            let mut relative_cursor = 0;
            let mut used_original_text_id = false;
            while let Some((template, range)) = templates.get(*template_cursor) {
                if range.start >= node_end {
                    break;
                }
                if range.start < node_start || range.end > node_end {
                    return Err(template.node_type.clone());
                }
                let relative_start = range.start - node_start;
                let relative_end = range.end - node_start;
                if relative_start > relative_cursor {
                    push_split_text_node(
                        &mut next,
                        &child,
                        char_slice(&text, relative_cursor..relative_start)
                            .ok_or_else(|| template.node_type.clone())?,
                        &mut used_original_text_id,
                    );
                }
                next.push(template.node.clone());
                relative_cursor = relative_end;
                *template_cursor += 1;
            }
            if relative_cursor < char_count {
                push_split_text_node(
                    &mut next,
                    &child,
                    char_slice(&text, relative_cursor..char_count)
                        .expect("validated character range"),
                    &mut used_original_text_id,
                );
            }
            *plain_cursor = node_end;
        } else {
            replace_preserved_inline_ranges(&mut child, templates, plain_cursor, template_cursor)?;
            next.push(child);
        }
    }
    *children = next;
    Ok(())
}

fn push_split_text_node(
    children: &mut Vec<Value>,
    template: &Value,
    text: &str,
    used_original_id: &mut bool,
) {
    if text.is_empty() {
        return;
    }
    let mut node = template.clone();
    set_text_node_content(&mut node, text, (*used_original_id).then(node_id));
    *used_original_id = true;
    children.push(node);
}

fn char_slice(text: &str, range: Range<usize>) -> Option<&str> {
    let char_count = text.chars().count();
    if range.start > range.end || range.end > char_count {
        return None;
    }
    let start = char_to_byte_offset(text, range.start);
    let end = char_to_byte_offset(text, range.end);
    Some(&text[start..end])
}

#[derive(Clone)]
struct CommentMarkTemplate {
    node: Value,
    plain_range: Range<usize>,
}

pub fn contains_document_comment_marks(node: &Value) -> bool {
    node.get("type").and_then(Value::as_str) == Some("comment-mark")
        || node
            .get("children")
            .and_then(Value::as_array)
            .is_some_and(|children| children.iter().any(contains_document_comment_marks))
        || node
            .get("root")
            .is_some_and(contains_document_comment_marks)
}

fn reapply_document_comment_marks(existing: &Value, incoming: &Value) -> Result<Value, ()> {
    let mut cursor = 0;
    let mut templates = Vec::new();
    collect_comment_mark_templates(existing, &mut cursor, &mut templates, false)?;
    if templates.is_empty() {
        return Ok(merge_editable_node(existing, incoming));
    }
    let old_plain = lexical_plain_text(existing);
    let new_plain = lexical_plain_text(incoming);
    let mut mapped = templates
        .into_iter()
        .map(|template| {
            let plain_range = map_plain_range_through_edits(
                &old_plain,
                &new_plain,
                template.plain_range.clone(),
            )?;
            (plain_range.start < plain_range.end).then_some((template, plain_range))
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(())?;
    mapped.sort_by_key(|(_, range)| (range.start, range.end));
    if mapped
        .windows(2)
        .any(|pair| pair[0].1.end > pair[1].1.start)
    {
        return Err(());
    }

    let merged = merge_editable_node(existing, incoming);
    let mut state = json!({
        "root": {
            "$": {"id": "root"},
            "children": [merged],
            "direction": null,
            "format": "",
            "indent": 0,
            "type": "root",
            "version": 1
        }
    });
    for (template, plain_range) in mapped {
        let markdown_range = plain_range_to_markdown_range(&state, plain_range).ok_or(())?;
        let temporary_id = format!("GPUI-COMMENT-{}", Uuid::new_v4());
        let thread_id = template
            .node
            .get("threadId")
            .and_then(Value::as_i64)
            .unwrap_or(-1);
        state = anchor_document_comment(&state, markdown_range, &temporary_id, thread_id)
            .map_err(|_| ())?;
        let mut replaced = 0;
        restore_comment_mark_template(&mut state, &temporary_id, &template.node, &mut replaced);
        if replaced == 0 {
            return Err(());
        }
    }
    state.pointer("/root/children/0").cloned().ok_or(())
}

fn collect_comment_mark_templates(
    node: &Value,
    plain_cursor: &mut usize,
    templates: &mut Vec<CommentMarkTemplate>,
    inside_comment: bool,
) -> Result<(), ()> {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    if matches!(
        kind,
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight"
    ) {
        *plain_cursor += node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .chars()
            .count();
        return Ok(());
    }
    if kind == "comment-mark" {
        if inside_comment {
            return Err(());
        }
        let start = *plain_cursor;
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                collect_comment_mark_templates(child, plain_cursor, templates, true)?;
            }
        }
        templates.push(CommentMarkTemplate {
            node: node.clone(),
            plain_range: start..*plain_cursor,
        });
        return Ok(());
    }
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        for child in children {
            collect_comment_mark_templates(child, plain_cursor, templates, inside_comment)?;
        }
    }
    Ok(())
}

fn lexical_plain_text(node: &Value) -> String {
    if matches!(
        node.get("type").and_then(Value::as_str).unwrap_or_default(),
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight"
    ) {
        return node
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
    }
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(lexical_plain_text)
        .collect()
}

#[derive(Clone, Copy)]
enum BoundaryAffinity {
    Before,
    After,
}

fn map_plain_range_through_edits(
    old: &str,
    new: &str,
    range: Range<usize>,
) -> Option<Range<usize>> {
    let diff = TextDiff::from_chars(old, new);
    let start = map_diff_boundary(&diff, range.start, BoundaryAffinity::Before)?;
    let end = map_diff_boundary(&diff, range.end, BoundaryAffinity::After)?;
    (start <= end).then_some(start..end)
}

fn map_diff_boundary(
    diff: &TextDiff<'_, '_, '_, str>,
    offset: usize,
    affinity: BoundaryAffinity,
) -> Option<usize> {
    for operation in diff.ops() {
        if matches!(operation.tag(), DiffTag::Insert) && operation.old_range().start == offset {
            return Some(match affinity {
                BoundaryAffinity::Before => operation.new_range().start,
                BoundaryAffinity::After => operation.new_range().end,
            });
        }
    }
    for operation in diff.ops() {
        let old = operation.old_range();
        let new = operation.new_range();
        if offset < old.start || offset > old.end || matches!(operation.tag(), DiffTag::Insert) {
            continue;
        }
        return Some(match operation.tag() {
            DiffTag::Equal => new.start + (offset - old.start),
            DiffTag::Delete => new.start,
            DiffTag::Replace => match affinity {
                BoundaryAffinity::Before => new.start,
                BoundaryAffinity::After => new.end,
            },
            DiffTag::Insert => unreachable!(),
        });
    }
    (offset == diff.old_slices().len()).then(|| diff.new_slices().len())
}

fn plain_range_to_markdown_range(state: &Value, range: Range<usize>) -> Option<Range<usize>> {
    let spans = lexical_text_spans(state);
    let mut plain_cursor = 0;
    let mut markdown_start = None;
    let mut markdown_end = None;
    for span in spans {
        let text = find_lexical_text(state, &span.node_id)?;
        let char_count = text.chars().count();
        let plain_end = plain_cursor + char_count;
        if markdown_start.is_none() && range.start >= plain_cursor && range.start < plain_end {
            markdown_start = Some(
                span.markdown_start
                    + text
                        .chars()
                        .take(range.start - plain_cursor)
                        .map(char::len_utf8)
                        .sum::<usize>(),
            );
        }
        if range.end > plain_cursor && range.end <= plain_end {
            markdown_end = Some(
                span.markdown_start
                    + text
                        .chars()
                        .take(range.end - plain_cursor)
                        .map(char::len_utf8)
                        .sum::<usize>(),
            );
        }
        plain_cursor = plain_end;
    }
    Some(markdown_start?..markdown_end?)
}

fn find_lexical_text<'a>(node: &'a Value, node_id: &str) -> Option<&'a str> {
    if node.pointer("/$/id").and_then(Value::as_str) == Some(node_id) {
        return node.get("text").and_then(Value::as_str);
    }
    node.get("children")
        .and_then(Value::as_array)
        .and_then(|children| {
            children
                .iter()
                .find_map(|child| find_lexical_text(child, node_id))
        })
        .or_else(|| {
            node.get("root")
                .and_then(|root| find_lexical_text(root, node_id))
        })
}

fn restore_comment_mark_template(
    node: &mut Value,
    temporary_id: &str,
    template: &Value,
    replaced: &mut usize,
) {
    let matches = node.get("type").and_then(Value::as_str) == Some("comment-mark")
        && node
            .get("ids")
            .and_then(Value::as_array)
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(temporary_id)));
    if matches {
        let children = node
            .get_mut("children")
            .and_then(Value::as_array_mut)
            .map(std::mem::take)
            .unwrap_or_default();
        let mut restored = template.clone();
        if let Some(object) = restored.as_object_mut() {
            object.insert("children".into(), Value::Array(children));
            if *replaced > 0
                && let Some(metadata) = object.get_mut("$").and_then(Value::as_object_mut)
            {
                metadata.insert("id".into(), node_id().into());
            }
        }
        *node = restored;
        *replaced += 1;
        return;
    }
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            restore_comment_mark_template(child, temporary_id, template, replaced);
        }
    }
    if let Some(root) = node.get_mut("root") {
        restore_comment_mark_template(root, temporary_id, template, replaced);
    }
}

/// Reads the Lexical node-state value used by Macro's pinned-properties plugin.
/// Older fixtures stored it directly on the root, so the fallback remains
/// accepted while all writes use the current `$` node-state envelope.
pub fn pinned_property_ids(state: &Value) -> Vec<String> {
    let root = state.get("root").unwrap_or(state);
    root.get("$")
        .and_then(|metadata| metadata.get("pinnedPropertyIds"))
        .or_else(|| root.get("pinnedPropertyIds"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

pub fn set_pinned_property_ids(state: &mut Value, property_ids: Vec<String>) {
    let root = if state.get("root").is_some() {
        state.get_mut("root")
    } else {
        Some(state)
    };
    let Some(root) = root.and_then(Value::as_object_mut) else {
        return;
    };
    let metadata = root.entry("$").or_insert_with(|| json!({ "id": "root" }));
    if let Some(metadata) = metadata.as_object_mut() {
        metadata.insert("pinnedPropertyIds".into(), json!(property_ids));
    }
    root.remove("pinnedPropertyIds");
}

fn normalize_projected_block(markdown: &str) -> &str {
    markdown.trim_end_matches(['\n', '\r', ' '])
}

fn is_native_editable_top_level(kind: &str) -> bool {
    matches!(
        kind,
        "paragraph"
            | "heading"
            | "quote"
            | "code"
            | "custom-code"
            | "horizontalrule"
            | "list"
            | "table"
            | "image"
            | "video"
            | "equation"
    )
}

fn is_native_rewritable_node(kind: &str) -> bool {
    matches!(
        kind,
        "root"
            | "paragraph"
            | "heading"
            | "quote"
            | "code"
            | "custom-code"
            | "horizontalrule"
            | "list"
            | "listitem"
            | "table"
            | "tablerow"
            | "tablecell"
            | "image"
            | "video"
            | "equation"
            | "link"
            | "autolink"
            | "text"
            | "code-highlight"
            | "unlinked-text"
            | "inline-search"
            | "linebreak"
    )
}

fn first_unsupported_descendant(node: &Value) -> Option<&str> {
    node.get("children")
        .and_then(Value::as_array)
        .and_then(|children| {
            children.iter().find_map(|child| {
                let kind = child.get("type").and_then(Value::as_str)?;
                if kind != "comment-mark"
                    && !is_preserved_inline_node(kind)
                    && !is_native_rewritable_node(kind)
                {
                    Some(kind)
                } else {
                    first_unsupported_descendant(child)
                }
            })
        })
}

/// Merge editable fields while carrying forward identity and unknown schema
/// extensions. When a node changes kind, only its Lexical identity metadata is
/// retained; kind-specific fields must not leak between node classes.
fn merge_editable_node(existing: &Value, incoming: &Value) -> Value {
    let existing_kind = existing.get("type").and_then(Value::as_str);
    let incoming_kind = incoming.get("type").and_then(Value::as_str);
    // Macro replaces Lexical's stock CodeNode with CustomCodeNode. Markdown
    // import necessarily produces `code`; keep the authoritative class while
    // applying its editable language/text fields and preserving extensions.
    if existing_kind == Some("custom-code") && incoming_kind == Some("code") {
        let mut compatible = incoming.clone();
        if let Some(object) = compatible.as_object_mut() {
            object.insert("type".into(), Value::String("custom-code".into()));
        }
        return merge_editable_node(existing, &compatible);
    }
    if matches!(
        existing_kind,
        Some("code-highlight" | "unlinked-text" | "inline-search")
    ) && incoming_kind == Some("text")
    {
        let mut compatible = incoming.clone();
        if let Some(object) = compatible.as_object_mut() {
            object.insert(
                "type".into(),
                Value::String(existing_kind.unwrap_or("text").into()),
            );
        }
        return merge_editable_node(existing, &compatible);
    }
    // The friendly native projection for a video is `[Video](url)`. Convert
    // that parsed one-link paragraph back to Macro's decorator node, changing
    // only its URL while retaining provider IDs, dimensions, constraints,
    // scale, controls, and future fields.
    if existing_kind == Some("video")
        && incoming_kind != Some("video")
        && let Some(compatible) = projected_video_node(existing, incoming)
    {
        return merge_editable_node(existing, &compatible);
    }
    if existing_kind != incoming_kind {
        let mut replacement = incoming.clone();
        preserve_node_metadata(existing, &mut replacement);
        return replacement;
    }

    let (Some(existing_object), Some(incoming_object)) =
        (existing.as_object(), incoming.as_object())
    else {
        return incoming.clone();
    };
    let mut merged = existing_object.clone();
    for (key, value) in incoming_object {
        if key != "$" && key != "children" {
            merged.insert(key.clone(), value.clone());
        }
    }
    if let Some(incoming_children) = incoming_object.get("children").and_then(Value::as_array) {
        let existing_children = existing_object
            .get("children")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let children = incoming_children
            .iter()
            .enumerate()
            .map(|(index, incoming_child)| {
                existing_children
                    .get(index)
                    .map(|existing_child| merge_editable_node(existing_child, incoming_child))
                    .unwrap_or_else(|| incoming_child.clone())
            })
            .collect();
        merged.insert("children".into(), Value::Array(children));
    }
    Value::Object(merged)
}

fn projected_video_node(existing: &Value, incoming: &Value) -> Option<Value> {
    let children = incoming.get("children")?.as_array()?;
    let [link] = children.as_slice() else {
        return None;
    };
    if incoming.get("type").and_then(Value::as_str) != Some("paragraph")
        || !matches!(
            link.get("type").and_then(Value::as_str),
            Some("link" | "autolink")
        )
        || descendant_text(std::slice::from_ref(link)).trim() != "Video"
    {
        return None;
    }
    let url = link.get("url")?.as_str()?;
    let mut compatible = existing.clone();
    compatible
        .as_object_mut()?
        .insert("url".into(), Value::String(url.into()));
    Some(compatible)
}

fn preserve_node_metadata(existing: &Value, replacement: &mut Value) {
    let Some(metadata) = existing.get("$").cloned() else {
        return;
    };
    if let Some(replacement) = replacement.as_object_mut() {
        replacement.insert("$".into(), metadata);
    }
}

#[derive(Debug)]
enum FrameKind {
    Paragraph,
    Heading { tag: String },
    Quote,
    Code { language: String },
    List { start: Option<u64> },
    Item { checked: Option<bool> },
    Link { url: String, title: String },
    Image { src: String },
    Table,
    TableHead,
    TableRow,
    TableCell,
    Transparent,
}

#[derive(Debug)]
struct LexicalFrame {
    kind: FrameKind,
    children: Vec<Value>,
}

impl LexicalFrame {
    fn new(kind: FrameKind) -> Self {
        Self {
            kind,
            children: Vec::new(),
        }
    }

    fn push(&mut self, node: Value) {
        self.children.push(node);
    }

    fn finish(self) -> Vec<Value> {
        let Self { kind, children } = self;
        match kind {
            FrameKind::Transparent => children,
            FrameKind::Paragraph => vec![element_node(
                "paragraph",
                Map::from_iter([
                    ("textFormat".into(), json!(0)),
                    ("textStyle".into(), json!("")),
                ]),
                children,
            )],
            FrameKind::Heading { tag } => vec![element_node(
                "heading",
                Map::from_iter([("tag".into(), json!(tag))]),
                children,
            )],
            FrameKind::Quote => vec![element_node(
                "quote",
                Map::new(),
                flatten_paragraphs(children),
            )],
            FrameKind::Code { language } => vec![element_node(
                "code",
                Map::from_iter([("language".into(), json!(language))]),
                flatten_paragraphs(children),
            )],
            FrameKind::List { start } => {
                let is_check = children.iter().any(|child| child.get("checked").is_some());
                let ordered = start.is_some();
                vec![element_node(
                    "list",
                    Map::from_iter([
                        (
                            "listType".into(),
                            json!(if is_check {
                                "check"
                            } else if ordered {
                                "number"
                            } else {
                                "bullet"
                            }),
                        ),
                        ("start".into(), json!(start.unwrap_or(1))),
                        ("tag".into(), json!(if ordered { "ol" } else { "ul" })),
                    ]),
                    children,
                )]
            }
            FrameKind::Item { checked } => {
                let mut fields = Map::from_iter([("value".into(), json!(1))]);
                if let Some(checked) = checked {
                    fields.insert("checked".into(), json!(checked));
                }
                vec![element_node(
                    "listitem",
                    fields,
                    flatten_paragraphs(children),
                )]
            }
            FrameKind::Link { url, title } => vec![element_node(
                "link",
                Map::from_iter([
                    ("url".into(), json!(url)),
                    (
                        "title".into(),
                        if title.is_empty() {
                            Value::Null
                        } else {
                            json!(title)
                        },
                    ),
                    ("rel".into(), Value::Null),
                    ("target".into(), Value::Null),
                ]),
                children,
            )],
            FrameKind::Image { src } => {
                let alt = descendant_text(&children);
                vec![decorator_node(
                    "image",
                    [
                        ("srcType", json!("url")),
                        ("id", json!("")),
                        ("url", json!(src)),
                        ("alt", json!(alt)),
                        ("width", json!(0)),
                        ("height", json!(0)),
                        ("scale", json!(1)),
                    ],
                )]
            }
            FrameKind::Table => vec![element_node("table", Map::new(), children)],
            FrameKind::TableHead => vec![element_node("tablerow", Map::new(), children)],
            FrameKind::TableRow => vec![element_node("tablerow", Map::new(), children)],
            FrameKind::TableCell => vec![element_node(
                "tablecell",
                Map::from_iter([
                    ("backgroundColor".into(), Value::Null),
                    ("colSpan".into(), json!(1)),
                    ("headerState".into(), json!(0)),
                    ("rowSpan".into(), json!(1)),
                ]),
                children,
            )],
        }
    }
}

fn finish_frame(stack: &mut Vec<LexicalFrame>, roots: &mut Vec<Value>) {
    let Some(frame) = stack.pop() else {
        return;
    };
    for node in frame.finish() {
        push_node(stack, roots, node);
    }
}

fn push_node(stack: &mut [LexicalFrame], roots: &mut Vec<Value>, node: Value) {
    if let Some(parent) = stack.last_mut() {
        parent.push(node);
    } else {
        roots.push(node);
    }
}

fn node_id() -> String {
    // Macro's Lexical IDs are opaque 12-hex-character identifiers. Calling
    // the operating-system RNG for every text and element node dominates
    // initial projection of large documents, so randomize one process-local
    // 48-bit prefix and advance it atomically for each node. This retains the
    // same wire shape and collision probability against imported documents,
    // while guaranteeing uniqueness within a process for 2^48 allocations.
    static NODE_ID_SEED: OnceLock<u64> = OnceLock::new();
    static NODE_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const MASK_48: u64 = (1_u64 << 48) - 1;
    let seed = *NODE_ID_SEED.get_or_init(|| Uuid::new_v4().as_u128() as u64 & MASK_48);
    let sequence = NODE_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{:012x}", seed.wrapping_add(sequence) & MASK_48)
}

fn base_node(kind: &str) -> Map<String, Value> {
    Map::from_iter([
        ("$".into(), json!({ "id": node_id() })),
        ("type".into(), json!(kind)),
        ("version".into(), json!(1)),
    ])
}

fn element_node(kind: &str, fields: Map<String, Value>, children: Vec<Value>) -> Value {
    let mut node = base_node(kind);
    node.extend(Map::from_iter([
        ("children".into(), Value::Array(children)),
        ("direction".into(), Value::Null),
        ("format".into(), json!("")),
        ("indent".into(), json!(0)),
    ]));
    node.extend(fields);
    Value::Object(node)
}

fn decorator_node<'a>(kind: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    let mut node = base_node(kind);
    node.extend(fields.into_iter().map(|(key, value)| (key.into(), value)));
    Value::Object(node)
}

fn text_node(text: &str, format: u32) -> Value {
    let mut node = base_node("text");
    node.extend(Map::from_iter([
        ("detail".into(), json!(0)),
        ("format".into(), json!(format)),
        ("mode".into(), json!("normal")),
        ("style".into(), json!("")),
        ("text".into(), json!(text)),
    ]));
    Value::Object(node)
}

fn flatten_paragraphs(children: Vec<Value>) -> Vec<Value> {
    children
        .into_iter()
        .flat_map(|child| {
            if child.get("type").and_then(Value::as_str) == Some("paragraph") {
                child
                    .get("children")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            } else {
                vec![child]
            }
        })
        .collect()
}

fn descendant_text(children: &[Value]) -> String {
    children
        .iter()
        .map(|child| {
            child
                .get("text")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| {
                    child
                        .get("children")
                        .and_then(Value::as_array)
                        .map(|children| descendant_text(children))
                        .unwrap_or_default()
                })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum RenderContext {
    Block,
    Inline,
}

/// Lexical node types registered by Macro's pinned `lexical-core/node-list.ts`.
/// Keep this manifest explicit: adding a web node without a native projection
/// must fail the compatibility audit instead of silently turning into JSON or
/// disappearing. `root` is included because it is present on the wire even
/// though Lexical registers it internally.
pub const MACRO_LEXICAL_NODE_TYPES: &[&str] = &[
    "root",
    "paragraph",
    "text",
    "code",
    "custom-code",
    "heading",
    "link",
    "autolink",
    "list",
    "listitem",
    "quote",
    "linebreak",
    "document-mention",
    "document-card",
    "paste",
    "user-mention",
    "contact-mention",
    "date-mention",
    "pr-mention",
    "group-mention",
    "inline-search",
    "unlinked-text",
    "code-highlight",
    "tab",
    "image",
    "video",
    "table",
    "tablecell",
    "tablerow",
    "completion",
    "mark",
    "comment-mark",
    "search-match",
    "equation",
    "snapshot",
    "horizontalrule",
    "diff",
    "diff-insert",
    "diff-delete",
    "diff-text",
    "html-render",
    "classed-block",
    "watermark",
    "theme-mention",
    "tag-mention",
    "unknown-mention",
    "await",
    "magic-chip",
    "agent-context",
];

fn render_children(node: &serde_json::Value, context: RenderContext) -> String {
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .map(|children| {
            if matches!(context, RenderContext::Inline) {
                return render_inline_children(children);
            }
            let separator = match context {
                RenderContext::Block => "\n\n",
                RenderContext::Inline => "",
            };
            children
                .iter()
                .map(|child| render_node(child, context))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(separator)
        })
        .unwrap_or_default()
}

enum InlineFragment {
    Formatted { text: String, format: u64 },
    Rendered(String),
}

fn render_inline_children(children: &[Value]) -> String {
    let mut fragments = Vec::new();
    for child in children {
        collect_inline_fragments(child, &mut fragments);
    }
    let mut rendered = String::new();
    let mut pending = None::<(u64, String)>;
    for fragment in fragments {
        match fragment {
            InlineFragment::Formatted { text, format } => {
                if let Some((pending_format, pending_text)) = pending.as_mut()
                    && *pending_format == format
                {
                    pending_text.push_str(&text);
                } else {
                    if let Some((format, text)) = pending.take() {
                        rendered.push_str(&render_text_with_format(text, format));
                    }
                    pending = Some((format, text));
                }
            }
            InlineFragment::Rendered(value) => {
                if let Some((format, text)) = pending.take() {
                    rendered.push_str(&render_text_with_format(text, format));
                }
                rendered.push_str(&value);
            }
        }
    }
    if let Some((format, text)) = pending {
        rendered.push_str(&render_text_with_format(text, format));
    }
    rendered
}

fn collect_inline_fragments(node: &Value, fragments: &mut Vec<InlineFragment>) {
    let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
    match kind {
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight" => {
            fragments.push(InlineFragment::Formatted {
                text: node
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                format: node
                    .get("format")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
            });
        }
        "mark" | "comment-mark" | "classed-block" | "comment" | "diff" | "diff-insert"
        | "diff-delete" => {
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    collect_inline_fragments(child, fragments);
                }
            }
        }
        _ => fragments.push(InlineFragment::Rendered(render_node(
            node,
            RenderContext::Inline,
        ))),
    }
}

fn render_node(node: &serde_json::Value, context: RenderContext) -> String {
    let kind = node
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    match kind {
        "text" | "unlinked-text" | "inline-search" | "diff-text" | "code-highlight" => {
            render_text(node)
        }
        "linebreak" => "  \n".into(),
        "tab" => "\t".into(),
        "paragraph" | "root" | "listitem" | "tablecell" | "tablerow" | "mark" | "comment-mark"
        | "classed-block" | "comment" | "search-match" | "diff" | "diff-insert" | "diff-delete" => {
            render_children(node, RenderContext::Inline)
        }
        "completion" => {
            let descendants = render_children(node, RenderContext::Inline);
            if descendants.is_empty() {
                node.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            } else {
                descendants
            }
        }
        "heading" => {
            let level = node
                .get("tag")
                .and_then(serde_json::Value::as_str)
                .and_then(|tag| tag.strip_prefix('h'))
                .and_then(|level| level.parse::<usize>().ok())
                .unwrap_or(1)
                .clamp(1, 6);
            format!(
                "{} {}",
                "#".repeat(level),
                render_children(node, RenderContext::Inline)
            )
        }
        "quote" => render_children(node, RenderContext::Inline)
            .lines()
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "code" => {
            let language = node
                .get("language")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            render_code_block(language, node)
        }
        "custom-code" => {
            let language = node
                .get("language")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            render_code_block(language, node)
        }
        "horizontalrule" => "---".into(),
        "link" | "autolink" => {
            let label = render_children(node, RenderContext::Inline);
            let url = node
                .get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if url.is_empty() {
                label
            } else {
                format!("[{label}]({url})")
            }
        }
        "list" => render_list(node),
        "image" => format!(
            "![{}]({})",
            node.get("alt")
                .or_else(|| node.get("altText"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default(),
            node.get("url")
                .or_else(|| node.get("src"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
        ),
        "video" => {
            let url = node
                .get("url")
                .or_else(|| node.get("src"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if url.is_empty() {
                "Video".into()
            } else {
                format!("[Video]({url})")
            }
        }
        "html-render" => {
            let html = node
                .get("html")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            format!("```html\n{html}\n```")
        }
        "document-card" => format!(
            "@{}",
            node.get("documentName")
                .or_else(|| node.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Document")
        ),
        "paste" => {
            let content = node
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            fenced_literal("text", content)
        }
        "equation" => {
            let equation = node
                .get("equation")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if node
                .get("inline")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                format!("${equation}$")
            } else {
                format!("$$\n{equation}\n$$")
            }
        }
        "table" => render_table(node),
        "date-mention" => format!(
            "@{}",
            node.get("displayFormat")
                .or_else(|| node.get("date"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("date")
        ),
        "tag-mention" => format!(
            "#{}",
            node.get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tag")
        ),
        // Macro marks agent context as private, hidden, excluded from copy,
        // and excluded from search. Never expose its persisted prompt text in
        // the native visual/clipboard projection.
        "agent-context" | "await" => String::new(),
        "snapshot" => format!(
            "@{}",
            node.get("documentName")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Snapshot")
        ),
        "watermark" => node
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        "magic-chip" => {
            let status = node
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("agent");
            format!("[Macro agent · {status}]")
        }
        kind if kind.ends_with("-mention") || kind == "mention" => {
            let label = [
                "displayName",
                "documentName",
                "name",
                "groupAlias",
                "label",
                "email",
                "text",
                "title",
            ]
            .into_iter()
            .find_map(|key| node.get(key).and_then(serde_json::Value::as_str))
            .unwrap_or(kind);
            format!("@{label}")
        }
        _ => {
            let descendants = render_children(
                node,
                if matches!(context, RenderContext::Block) {
                    RenderContext::Block
                } else {
                    RenderContext::Inline
                },
            );
            if descendants.is_empty() {
                let text = node
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if !text.is_empty() {
                    text
                } else if kind.is_empty() {
                    "[Unsupported Macro block]".into()
                } else if matches!(context, RenderContext::Block) {
                    format!("[Unsupported Macro block · {kind}]")
                } else {
                    format!("[Unsupported Macro inline · {kind}]")
                }
            } else {
                descendants
            }
        }
    }
}

fn render_code_block(language: &str, node: &Value) -> String {
    let contents = render_children(node, RenderContext::Inline);
    let contents = contents.trim_end();
    match language {
        "macro-frontmatter-yaml" => format!("---\n{contents}\n---"),
        "macro-frontmatter-toml" => format!("+++\n{contents}\n+++"),
        _ => fenced_literal(language, contents),
    }
}

fn fenced_literal(language: &str, contents: &str) -> String {
    let longest_run = contents
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat(longest_run.max(2) + 1);
    format!("{fence}{language}\n{}\n{fence}", contents.trim_end())
}

fn render_text(node: &serde_json::Value) -> String {
    let text = node
        .get("text")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let format = node
        .get("format")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    render_text_with_format(text, format)
}

fn render_text_with_format(mut text: String, format: u64) -> String {
    if format & 16 != 0 {
        text = format!("`{text}`");
    }
    if format & 1 != 0 {
        text = format!("**{text}**");
    }
    if format & 2 != 0 {
        text = format!("*{text}*");
    }
    if format & 4 != 0 {
        text = format!("~~{text}~~");
    }
    if format & 8 != 0 {
        text = format!("<u>{text}</u>");
    }
    if format & 32 != 0 {
        text = format!("<sub>{text}</sub>");
    }
    if format & 64 != 0 {
        text = format!("<sup>{text}</sup>");
    }
    if format & 128 != 0 {
        text = format!("=={text}==");
    }
    text
}

fn render_list(node: &serde_json::Value) -> String {
    let list_type = node
        .get("listType")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("bullet");
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let marker = match list_type {
                        "number" => format!("{}. ", index + 1),
                        "check" => format!(
                            "- [{}] ",
                            if item
                                .get("checked")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(false)
                            {
                                "x"
                            } else {
                                " "
                            }
                        ),
                        _ => "- ".into(),
                    };
                    let content = render_children(item, RenderContext::Inline);
                    format!("{marker}{}", content.replace('\n', "\n  "))
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn render_table(node: &serde_json::Value) -> String {
    let rows = node
        .get("children")
        .and_then(serde_json::Value::as_array)
        .map(|rows| {
            rows.iter()
                .map(|row| {
                    row.get("children")
                        .and_then(serde_json::Value::as_array)
                        .map(|cells| {
                            cells
                                .iter()
                                .map(|cell| render_children(cell, RenderContext::Inline))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut rendered = Vec::new();
    for (index, mut row) in rows.into_iter().enumerate() {
        row.resize(columns, String::new());
        rendered.push(format!("| {} |", row.join(" | ")));
        if index == 0 {
            rendered.push(format!("| {} |", vec!["---"; columns].join(" | ")));
        }
    }
    rendered.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{EntityInputHandler as _, TestAppContext};
    use serde_json::json;

    fn init_gpui_editor_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assets::Assets.load_test_fonts(cx);
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            <theme_settings::ThemeSettings as settings::Settings>::register(cx);
            <editor::EditorSettings as settings::Settings>::register(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    #[test]
    fn lexical_node_ids_keep_the_macro_wire_shape_and_are_process_unique() {
        let ids = (0..100_000).map(|_| node_id()).collect::<Vec<_>>();
        assert!(
            ids.iter()
                .all(|id| { id.len() == 12 && id.bytes().all(|byte| byte.is_ascii_hexdigit()) })
        );
        let unique = ids.iter().collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), ids.len());
    }

    #[gpui::test]
    fn inline_block_editor_writes_through_the_authoritative_document(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let original = "# Brief\n\nOriginal paragraph\n\n- [ ] Ship it";
        let (document, cx) = cx.add_window_view(move |window, cx| {
            NativeDocumentEditor::new(
                "document-1".into(),
                "Brief".into(),
                original.into(),
                false,
                window,
                cx,
            )
        });
        cx.run_until_parked();
        assert!(!document.read_with(cx, |document, _| document.is_dirty()));
        let initial_block_ids = document.read_with(cx, |document, cx| {
            document
                .block_summaries(cx)
                .into_iter()
                .map(|block| block.stable_id)
                .collect::<Vec<_>>()
        });

        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(1, window, cx));
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());
        let inline_length = inline_editor.read_with(cx, |editor, cx| editor.text(cx).len());
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(inline_length),
                    "Edited in place",
                )],
                cx,
            );
        });
        cx.run_until_parked();

        document.read_with(cx, |document, cx| {
            assert_eq!(
                document.text(cx),
                "# Brief\n\nEdited in place\n\n- [ ] Ship it"
            );
            assert!(document.is_dirty());
        });
        document.update(cx, |document, cx| {
            assert_eq!(document.active_block_index(cx), Some(1));
        });

        inline_editor.update_in(cx, |editor, window, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([Point::new(0, 0)..Point::new(0, 6)]),
            );
        });
        document.update_in(cx, |document, window, cx| {
            assert!(document.apply_markdown_command(MarkdownCommand::Bold, window, cx));
        });
        cx.run_until_parked();
        document.read_with(cx, |document, cx| {
            assert_eq!(
                document.text(cx),
                "# Brief\n\n**Edited** in place\n\n- [ ] Ship it"
            );
            assert!(document.inline_block_editing());
        });

        let inline_length = inline_editor.read_with(cx, |editor, cx| editor.text(cx).len());
        inline_editor.update_in(cx, |editor, window, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(inline_length),
                    "/quote",
                )],
                cx,
            );
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| selections.select_ranges([Point::new(0, 6)..Point::new(0, 6)]),
            );
        });
        cx.run_until_parked();
        document.update(cx, |document, cx| {
            let slash = document
                .slash_command_context(cx)
                .expect("inline slash command context");
            assert_eq!(slash.query, "quote");
        });
        document.update_in(cx, |document, window, cx| {
            assert!(document.apply_slash_command(MarkdownCommand::Quote, window, cx));
        });
        cx.run_until_parked();
        inline_editor.update(cx, |editor, cx| {
            editor.edit([(MultiBufferOffset(2)..MultiBufferOffset(2), "Quoted")], cx);
        });
        cx.run_until_parked();
        document.read_with(cx, |document, cx| {
            assert_eq!(document.text(cx), "# Brief\n\n> Quoted\n\n- [ ] Ship it");
            assert!(document.inline_block_editing());
            assert_eq!(
                document
                    .block_summaries(cx)
                    .into_iter()
                    .map(|block| block.stable_id)
                    .collect::<Vec<_>>(),
                initial_block_ids
            );
        });

        let inline_length = inline_editor.read_with(cx, |editor, cx| editor.text(cx).len());
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(MultiBufferOffset(0)..MultiBufferOffset(inline_length), "")],
                cx,
            );
        });
        cx.run_until_parked();
        document.read_with(cx, |document, cx| {
            assert!(!document.inline_block_editing());
            assert_eq!(document.text(cx), "# Brief\n\n\n\n- [ ] Ship it");
            assert_eq!(
                document
                    .block_summaries(cx)
                    .into_iter()
                    .map(|block| block.stable_id)
                    .collect::<Vec<_>>(),
                vec![initial_block_ids[0].clone(), initial_block_ids[2].clone()],
                "deleting one native block must not transfer its identity to its neighbor"
            );
        });
    }

    #[gpui::test]
    fn inline_block_editor_preserves_ime_composition_and_groups_undo(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let (document, cx) = cx.add_window_view(|window, cx| {
            NativeDocumentEditor::new(
                "document-ime".into(),
                "IME".into(),
                "Seed".into(),
                false,
                window,
                cx,
            )
        });
        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(0, window, cx));
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());

        inline_editor.update_in(cx, |editor, window, cx| {
            editor.replace_and_mark_text_in_range(Some(0..4), "に", None, window, cx);
            editor.replace_and_mark_text_in_range(Some(0..1), "日本", None, window, cx);
            editor.replace_text_in_range(None, "日本語", window, cx);
        });
        cx.run_until_parked();
        document.read_with(cx, |document, cx| {
            assert_eq!(document.text(cx), "日本語");
            assert!(document.inline_block_editing());
        });

        inline_editor.update_in(cx, |editor, window, cx| {
            editor.undo(&Default::default(), window, cx);
        });
        cx.run_until_parked();
        document.read_with(cx, |document, cx| {
            assert_eq!(document.text(cx), "Seed");
            assert!(document.inline_block_editing());
        });
    }

    #[gpui::test]
    fn inline_undo_history_does_not_cross_block_sessions(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let (document, cx) = cx.add_window_view(|window, cx| {
            NativeDocumentEditor::new(
                "document-undo-scope".into(),
                "Undo scope".into(),
                "First\n\nSecond".into(),
                false,
                window,
                cx,
            )
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());

        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(0, window, cx));
        });
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(MultiBufferOffset(5)..MultiBufferOffset(5), " changed")],
                cx,
            );
        });
        cx.run_until_parked();
        document.update_in(cx, |document, window, cx| {
            document.finish_rich_inline_edit(window, cx);
            assert!(document.begin_inline_block_edit(1, window, cx));
        });
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(MultiBufferOffset(6)..MultiBufferOffset(6), " changed")],
                cx,
            );
        });
        cx.run_until_parked();
        inline_editor.update_in(cx, |editor, window, cx| {
            editor.undo(&Default::default(), window, cx);
        });
        cx.run_until_parked();

        document.read_with(cx, |document, cx| {
            assert_eq!(document.text(cx), "First changed\n\nSecond");
            assert!(document.inline_block_editing());
        });
    }

    #[gpui::test]
    fn inline_formatting_applies_to_every_native_selection(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let (document, cx) = cx.add_window_view(|window, cx| {
            NativeDocumentEditor::new(
                "document-multiselect".into(),
                "Multiselect".into(),
                "one two".into(),
                false,
                window,
                cx,
            )
        });
        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(0, window, cx));
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());
        inline_editor.update_in(cx, |editor, window, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::fit()),
                window,
                cx,
                |selections| {
                    selections.select_ranges([
                        Point::new(0, 0)..Point::new(0, 3),
                        Point::new(0, 4)..Point::new(0, 7),
                    ])
                },
            );
        });
        document.update_in(cx, |document, window, cx| {
            assert!(document.apply_markdown_command(MarkdownCommand::Bold, window, cx));
        });
        cx.run_until_parked();

        document.read_with(cx, |document, cx| {
            assert_eq!(document.text(cx), "**one** **two**");
        });
        inline_editor.update(cx, |editor, cx| {
            assert_eq!(editor.selection_offset_ranges(cx), vec![2..5, 10..13]);
        });
    }

    #[gpui::test]
    fn inline_block_editor_reconciles_non_overlapping_remote_edits(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let original = "# Brief\n\nOriginal paragraph\n\n- [ ] Ship it";
        let source = markdown_to_lexical_state(original);
        let remote =
            project_markdown_edits(&source, "# Brief\n\nOriginal paragraph\n\n- [x] Ship it")
                .expect("remote fixture projects onto the same Lexical state");
        let (document, cx) = cx.add_window_view(move |window, cx| {
            let mut document = NativeDocumentEditor::new(
                "document-1".into(),
                "Brief".into(),
                original.into(),
                false,
                window,
                cx,
            );
            document.mark_saved_with_lexical_state(source, cx);
            document
        });
        cx.run_until_parked();

        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(1, window, cx));
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());
        let length = inline_editor.read_with(cx, |editor, cx| editor.text(cx).len());
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(length),
                    "Local paragraph",
                )],
                cx,
            );
        });
        cx.run_until_parked();

        document.update_in(cx, |document, window, cx| {
            document.apply_remote_lexical_state(remote, window, cx);
        });
        cx.run_until_parked();

        document.read_with(cx, |document, cx| {
            assert_eq!(
                document.text(cx),
                "# Brief\n\nLocal paragraph\n\n- [x] Ship it"
            );
            assert!(document.is_dirty());
            assert!(document.remote_conflict().is_none());
            assert!(document.inline_block_editing());
        });
        inline_editor.read_with(cx, |editor, cx| {
            assert_eq!(editor.text(cx), "Local paragraph");
        });
    }

    #[gpui::test]
    fn overlapping_remote_edit_freezes_and_exits_inline_editing(cx: &mut TestAppContext) {
        init_gpui_editor_test(cx);
        let original = "# Brief\n\nOriginal paragraph\n\n- [ ] Ship it";
        let source = markdown_to_lexical_state(original);
        let remote =
            project_markdown_edits(&source, "# Brief\n\nRemote paragraph\n\n- [ ] Ship it")
                .expect("remote fixture projects onto the same Lexical state");
        let (document, cx) = cx.add_window_view(move |window, cx| {
            let mut document = NativeDocumentEditor::new(
                "document-1".into(),
                "Brief".into(),
                original.into(),
                false,
                window,
                cx,
            );
            document.mark_saved_with_lexical_state(source, cx);
            document
        });
        cx.run_until_parked();

        document.update_in(cx, |document, window, cx| {
            assert!(document.begin_inline_block_edit(1, window, cx));
        });
        let inline_editor =
            document.read_with(cx, |document, _| document.rich_inline_editor.clone());
        let length = inline_editor.read_with(cx, |editor, cx| editor.text(cx).len());
        inline_editor.update(cx, |editor, cx| {
            editor.edit(
                [(
                    MultiBufferOffset(0)..MultiBufferOffset(length),
                    "Local paragraph",
                )],
                cx,
            );
        });
        cx.run_until_parked();

        document.update_in(cx, |document, window, cx| {
            document.apply_remote_lexical_state(remote, window, cx);
        });
        cx.run_until_parked();

        document.read_with(cx, |document, cx| {
            assert_eq!(
                document.text(cx),
                "# Brief\n\nLocal paragraph\n\n- [ ] Ship it"
            );
            assert!(!document.inline_block_editing());
            let conflict = document.remote_conflict().expect("overlap is surfaced");
            assert_eq!(conflict.local, document.text(cx));
            assert_eq!(
                conflict.remote,
                "# Brief\n\nRemote paragraph\n\n- [ ] Ship it"
            );
        });
    }

    #[test]
    fn rich_preview_keyboard_commands_ignore_modified_shortcuts() {
        assert_eq!(
            rich_preview_command("up", false),
            Some(RichPreviewCommand::Previous)
        );
        assert_eq!(
            rich_preview_command("j", false),
            Some(RichPreviewCommand::Next)
        );
        assert_eq!(
            rich_preview_command("home", false),
            Some(RichPreviewCommand::First)
        );
        assert_eq!(
            rich_preview_command("end", false),
            Some(RichPreviewCommand::Last)
        );
        assert_eq!(
            rich_preview_command("enter", false),
            Some(RichPreviewCommand::Edit)
        );
        assert_eq!(rich_preview_command("j", true), None);
        assert_eq!(rich_preview_command("escape", false), None);
    }

    #[test]
    fn rich_preview_opens_only_network_links() {
        assert_eq!(
            safe_markdown_external_url("https://macro.com/docs?q=gpui"),
            Some("https://macro.com/docs?q=gpui".into())
        );
        assert_eq!(
            safe_markdown_external_url("http://localhost:3000/document/1"),
            Some("http://localhost:3000/document/1".into())
        );
        for blocked in [
            "javascript:alert(1)",
            "file:///Users/example/secret.txt",
            "macro://document/private",
            "data:text/html,unsafe",
            "/relative/path",
            "https://",
        ] {
            assert_eq!(safe_markdown_external_url(blocked), None, "{blocked}");
        }
    }

    #[test]
    fn snippet_typeahead_requires_macro_trigger_boundaries_and_avoids_code() {
        assert_eq!(
            document_snippet_context(";standup", 8),
            Some(DocumentSnippetContext {
                range: 0..8,
                query: "standup".into(),
            })
        );
        assert_eq!(
            document_snippet_context("Plan ;daily later", 11),
            Some(DocumentSnippetContext {
                range: 5..11,
                query: "daily".into(),
            })
        );
        assert!(document_snippet_context("Plan;daily", 10).is_none());
        assert!(document_snippet_context(";;daily", 7).is_none());
        assert!(document_snippet_context("; daily", 7).is_none());
        assert!(document_snippet_context("`code ;daily`", 12).is_none());
        assert!(document_snippet_context("```md\n;daily\n```", 12).is_none());
        assert!(document_snippet_context(";dailySuffix", 6).is_none());
    }

    #[test]
    fn snippet_typeahead_keyboard_contract_uses_unmodified_navigation_keys() {
        assert_eq!(
            snippet_typeahead_command("up", false),
            Some(SnippetTypeaheadCommand::Previous)
        );
        assert_eq!(
            snippet_typeahead_command("down", false),
            Some(SnippetTypeaheadCommand::Next)
        );
        assert_eq!(
            snippet_typeahead_command("enter", false),
            Some(SnippetTypeaheadCommand::Accept)
        );
        assert_eq!(
            snippet_typeahead_command("escape", false),
            Some(SnippetTypeaheadCommand::Dismiss)
        );
        assert_eq!(snippet_typeahead_command("tab", false), None);
        assert_eq!(snippet_typeahead_command("down", true), None);
    }

    #[test]
    fn snippet_insertion_preserves_typed_mentions_and_existing_unknown_nodes() {
        let source = json!({
            "root": {
                "$": {"id": "root"},
                "type": "root",
                "version": 1,
                "children": [
                    {
                        "$": {"id": "before"},
                        "type": "paragraph",
                        "version": 1,
                        "children": [{
                            "$": {"id": "before-text"},
                            "type": "text",
                            "version": 1,
                            "text": "Keep",
                            "format": 0,
                            "detail": 0,
                            "mode": "normal",
                            "style": ""
                        }],
                        "direction": null,
                        "format": "",
                        "indent": 0
                    },
                    {
                        "$": {"id": "future-node"},
                        "type": "future-widget",
                        "version": 9,
                        "futureField": {"must": "survive"},
                        "children": [{
                            "$": {"id": "future-text"},
                            "type": "text",
                            "version": 1,
                            "text": "Opaque",
                            "format": 0,
                            "detail": 0,
                            "mode": "normal",
                            "style": ""
                        }]
                    },
                    {
                        "$": {"id": "trigger"},
                        "type": "paragraph",
                        "version": 1,
                        "children": [{
                            "$": {"id": "trigger-text"},
                            "type": "text",
                            "version": 1,
                            "text": ";daily",
                            "format": 0,
                            "detail": 0,
                            "mode": "normal",
                            "style": ""
                        }],
                        "direction": null,
                        "format": "",
                        "indent": 0
                    }
                ]
            }
        });
        let snippet = json!({
            "root": {
                "$": {"id": "snippet-root"},
                "type": "root",
                "version": 1,
                "children": [
                    {
                        "$": {"id": "snippet-heading"},
                        "type": "heading",
                        "version": 1,
                        "tag": "h2",
                        "children": [{
                            "$": {"id": "snippet-title"},
                            "type": "text",
                            "version": 1,
                            "text": "Daily update",
                            "format": 0,
                            "detail": 0,
                            "mode": "normal",
                            "style": ""
                        }],
                        "direction": null,
                        "format": "",
                        "indent": 0
                    },
                    {
                        "$": {"id": "snippet-body"},
                        "type": "paragraph",
                        "version": 1,
                        "children": [{
                            "$": {"id": "snippet-mention"},
                            "type": "user-mention",
                            "version": 1,
                            "userId": "user-1",
                            "email": "nick@example.com",
                            "displayName": "Nick",
                            "mentionUuid": "mention-1"
                        }],
                        "direction": null,
                        "format": "",
                        "indent": 0
                    }
                ]
            }
        });
        let current = lexical_state_to_markdown(&source);
        assert_eq!(current, "Keep\n\nOpaque\n\n;daily");
        let start = current.find(";daily").unwrap();
        let inserted = insert_snippet_lexical_state(
            Some(&source),
            &current,
            start..start + ";daily".len(),
            &snippet,
        )
        .unwrap();
        assert_eq!(
            inserted.markdown,
            "Keep\n\nOpaque\n\n## Daily update\n\n@Nick"
        );
        assert_eq!(inserted.caret, inserted.markdown.len());
        let future = inserted.state["root"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["type"] == "future-widget")
            .unwrap();
        assert_eq!(future["$"], json!({"id": "future-node"}));
        assert_eq!(future["futureField"], json!({"must": "survive"}));
        let wire = lexical_state_to_macro_markdown(&inserted.state);
        assert!(wire.contains("<m-user-mention>"));
        assert!(wire.contains("\"mentionUuid\":\"mention-1\""));
    }

    #[test]
    fn single_paragraph_snippet_flows_inline_at_the_trigger() {
        let current = "Before ;sign after";
        let source = markdown_to_lexical_state(current);
        let snippet = markdown_to_lexical_state("**Nick Gardner**");
        let start = current.find(";sign").unwrap();
        let inserted = insert_snippet_lexical_state(
            Some(&source),
            current,
            start..start + ";sign".len(),
            &snippet,
        )
        .unwrap();
        assert_eq!(inserted.markdown, "Before **Nick Gardner** after");
        assert_eq!(
            &inserted.markdown[..inserted.caret],
            "Before **Nick Gardner**"
        );
    }

    #[test]
    fn native_markdown_commands_transform_the_smallest_safe_range() {
        let markdown = "Plan\nShip it\nLater";
        let start = markdown.find("Ship").unwrap();
        let edit = markdown_command_edit(
            markdown,
            start..start + "Ship it".len(),
            MarkdownCommand::Heading(2),
        );
        assert_eq!(edit.range, 5..12);
        assert_eq!(edit.replacement, "## Ship it");

        let edit = markdown_command_edit(
            "one\ntwo",
            0.."one\ntwo".len(),
            MarkdownCommand::NumberedList,
        );
        assert_eq!(edit.replacement, "1. one\n2. two");

        let edit = markdown_command_edit("- [x] done", 0..11, MarkdownCommand::Paragraph);
        assert_eq!(edit.replacement, "done");

        let edit = markdown_command_edit("", 0..0, MarkdownCommand::Quote);
        assert_eq!(edit.replacement, "> ");
        assert_eq!(edit.selection, 2..2);
    }

    #[test]
    fn native_inline_and_insert_commands_leave_useful_selections() {
        let edit = markdown_command_edit("hello", 0..5, MarkdownCommand::Bold);
        assert_eq!(edit.replacement, "**hello**");
        assert_eq!(edit.selection, 2..7);

        let edit = markdown_command_edit("", 0..0, MarkdownCommand::Link);
        assert_eq!(edit.replacement, "[link text](https://)");
        assert_eq!(&edit.replacement[edit.selection.clone()], "https://");

        let edit = markdown_command_edit("", 0..0, MarkdownCommand::Table);
        assert!(edit.replacement.contains("| --- | --- |"));
        assert_eq!(&edit.replacement[edit.selection], "Column 1");

        let edit = markdown_command_edit("", 0..0, MarkdownCommand::Video);
        assert_eq!(edit.replacement, "[Video](https://)");
        assert_eq!(&edit.replacement[edit.selection], "https://");

        let edit = markdown_command_edit("é", 1..1, MarkdownCommand::Italic);
        assert_eq!(edit.range, 0..0, "invalid UTF-8 offsets are floored safely");
    }

    #[test]
    fn native_multi_selection_commands_shift_ranges_and_deduplicate_blocks() {
        let edit =
            markdown_multi_command_edit("one two", &[0..3, 4..7], MarkdownCommand::Strikethrough)
                .unwrap();
        assert_eq!(edit.text, "~~one~~ ~~two~~");
        assert_eq!(edit.selections, vec![2..5, 10..13]);
        assert_eq!(edit.edits.len(), 2);

        let heading =
            markdown_multi_command_edit("one two", &[0..0, 7..7], MarkdownCommand::Heading(1))
                .unwrap();
        assert_eq!(heading.text, "# one two");
        assert_eq!(heading.edits.len(), 1);
        assert_eq!(heading.selections, vec![9..9]);
    }

    #[test]
    fn structure_commands_indent_and_reorder_complete_markdown_blocks() {
        let indent = markdown_structure_command_edit(
            "- one\n- two\nafter",
            0..11,
            DocumentStructureCommand::Indent,
        )
        .unwrap();
        assert_eq!(indent.range, 0..11);
        assert_eq!(indent.replacement, "  - one\n  - two");
        let outdent = markdown_structure_command_edit(
            "  - one\n\t- two",
            0..16,
            DocumentStructureCommand::Outdent,
        )
        .unwrap();
        assert_eq!(outdent.replacement, "- one\n- two");

        let text = "Alpha\n\nBravo\n\nCharlie";
        let up =
            markdown_structure_command_edit(text, 7..12, DocumentStructureCommand::MoveBlockUp)
                .unwrap();
        assert_eq!(up.range, 0..12);
        assert_eq!(up.replacement, "Bravo\n\nAlpha");
        assert_eq!(up.selection, 0..5);
        let mut moved_up = text.to_owned();
        moved_up.replace_range(up.range, &up.replacement);
        assert_eq!(moved_up, "Bravo\n\nAlpha\n\nCharlie");

        let down =
            markdown_structure_command_edit(text, 7..12, DocumentStructureCommand::MoveBlockDown)
                .unwrap();
        assert_eq!(down.range, 7..21);
        assert_eq!(down.replacement, "Charlie\n\nBravo");
        assert_eq!(down.selection, 16..21);
        let mut moved_down = text.to_owned();
        moved_down.replace_range(down.range, &down.replacement);
        assert_eq!(moved_down, "Alpha\n\nCharlie\n\nBravo");

        assert!(
            markdown_structure_command_edit(text, 0..5, DocumentStructureCommand::MoveBlockUp,)
                .is_none()
        );
    }

    #[test]
    fn block_navigator_summarizes_and_moves_across_multiple_positions() {
        let text = "# Alpha\nbody\n\n- Bravo item\n\n```rust\nfn charlie() {}\n```";
        assert_eq!(
            markdown_block_summaries(text),
            vec![
                DocumentBlockSummary {
                    stable_id: "native-block-0".into(),
                    index: 0,
                    line: 0,
                    range: 0..7,
                    kind: DocumentBlockKind::Heading,
                    preview: "Alpha".into(),
                },
                DocumentBlockSummary {
                    stable_id: "native-block-1".into(),
                    index: 1,
                    line: 1,
                    range: 8..12,
                    kind: DocumentBlockKind::Paragraph,
                    preview: "body".into(),
                },
                DocumentBlockSummary {
                    stable_id: "native-block-2".into(),
                    index: 2,
                    line: 3,
                    range: 14..26,
                    kind: DocumentBlockKind::BulletedList,
                    preview: "Bravo item".into(),
                },
                DocumentBlockSummary {
                    stable_id: "native-block-3".into(),
                    index: 3,
                    line: 5,
                    range: 28..55,
                    kind: DocumentBlockKind::Code,
                    preview: "rust".into(),
                },
            ]
        );
        let edit = markdown_move_block_to_edit(text, 0, 2).unwrap();
        assert_eq!(
            edit.replacement,
            "body\n- Bravo item\n\n# Alpha\n\n```rust\nfn charlie() {}\n```"
        );
        assert_eq!(&edit.replacement[edit.selection], "# Alpha");
        let reverse = markdown_move_block_to_edit(&edit.replacement, 2, 0).unwrap();
        assert_eq!(reverse.replacement, text);
        assert!(markdown_move_block_to_edit(text, 1, 1).is_none());
        assert!(markdown_move_block_to_edit(text, 9, 0).is_none());
    }

    #[test]
    fn block_ranges_follow_markdown_structure_instead_of_blank_lines() {
        let text = concat!(
            "# Heading\n",
            "Body without a blank separator\n\n",
            "```rust\n",
            "fn first() {}\n\n",
            "fn second() {}\n",
            "```\n\n",
            "- first item\n\n",
            "- second item\n\n",
            "> quoted first\n",
            ">\n",
            "> quoted second\n\n",
            "| Name | State |\n",
            "| --- | --- |\n",
            "| Editor | Native |",
        );
        let blocks = markdown_block_ranges(text)
            .into_iter()
            .map(|range| &text[range])
            .collect::<Vec<_>>();
        assert_eq!(
            blocks,
            vec![
                "# Heading",
                "Body without a blank separator",
                "```rust\nfn first() {}\n\nfn second() {}\n```",
                "- first item\n\n- second item",
                "> quoted first\n>\n> quoted second",
                "| Name | State |\n| --- | --- |\n| Editor | Native |",
            ]
        );
    }

    #[test]
    fn moving_a_compound_block_preserves_its_internal_blank_lines() {
        let text = "Before\n\n```rust\nfn first() {}\n\nfn second() {}\n```\n\nAfter";
        let edit = markdown_move_block_to_edit(text, 1, 0).unwrap();
        assert_eq!(
            edit.replacement,
            "```rust\nfn first() {}\n\nfn second() {}\n```\n\nBefore\n\nAfter"
        );
        assert_eq!(
            &edit.replacement[edit.selection],
            "```rust\nfn first() {}\n\nfn second() {}\n```"
        );
    }

    #[test]
    fn block_summaries_reuse_macro_lexical_ids_and_classify_controls() {
        let source = json!({
            "root": {
                "children": [
                    {"$": {"id": "heading-42"}, "type": "heading"},
                    {"$": {"id": "task-list-7"}, "type": "list"},
                    {"$": {"id": "video-9"}, "type": "video"}
                ]
            }
        });
        let blocks = markdown_block_summaries_with_source(
            "## Roadmap\n\n- [ ] Ship native editor\n\n[Video](https://example.com/demo.mp4)",
            Some(&source),
        );
        assert_eq!(
            blocks
                .iter()
                .map(|block| (block.stable_id.as_str(), block.kind))
                .collect::<Vec<_>>(),
            vec![
                ("heading-42", DocumentBlockKind::Heading),
                ("task-list-7", DocumentBlockKind::Checklist),
                ("video-9", DocumentBlockKind::Media),
            ]
        );
        let locally_inserted = markdown_block_summaries_with_source(
            "New unsynced block\n\n## Roadmap\n\n- [ ] Ship native editor\n\n[Video](https://example.com/demo.mp4)",
            Some(&source),
        );
        assert!(
            locally_inserted
                .iter()
                .enumerate()
                .all(|(index, block)| block.stable_id == format!("native-block-{index}")),
            "positional Lexical IDs must not be assigned to a structurally different local document"
        );
    }

    #[test]
    fn document_search_supports_case_word_and_regex_modes() {
        let text = "Macro macroscopic MACRO café Café";
        assert_eq!(
            document_search_matches(text, "macro", DocumentSearchOptions::default()).unwrap(),
            vec![0..5, 6..11, 18..23]
        );
        assert_eq!(
            document_search_matches(
                text,
                "macro",
                DocumentSearchOptions {
                    whole_word: true,
                    ..Default::default()
                }
            )
            .unwrap(),
            vec![0..5, 18..23]
        );
        assert_eq!(
            document_search_matches(
                text,
                r"caf.\b",
                DocumentSearchOptions {
                    regex: true,
                    ..Default::default()
                }
            )
            .unwrap(),
            vec![24..29, 30..35]
        );
        assert!(
            document_search_matches(
                text,
                "[",
                DocumentSearchOptions {
                    regex: true,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("Invalid regular expression")
        );
    }

    #[test]
    fn document_replace_all_expands_regex_captures_but_keeps_plain_dollars_literal() {
        let result = replace_all_document_matches(
            "Gardner, Nick; Hauer, Jordan",
            r"(\w+), (\w+)",
            "$2 $1",
            DocumentSearchOptions {
                regex: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.text, "Nick Gardner; Jordan Hauer");
        assert_eq!(result.replacements, 2);

        let result = replace_all_document_matches(
            "price priceless price",
            "price",
            "$5",
            DocumentSearchOptions {
                whole_word: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.text, "$5 priceless $5");
        assert_eq!(result.replacements, 2);
    }

    #[test]
    fn slash_menu_only_activates_at_the_start_of_a_line_and_filters_commands() {
        assert_eq!(
            document_slash_context("Plan\n  /head", "Plan\n  /head".len()),
            Some(DocumentSlashContext {
                range: 7..12,
                query: "head".into(),
            })
        );
        assert!(document_slash_context("Visit https://macro.com", 19).is_none());
        assert!(document_slash_context("text /table", 11).is_none());
        assert!(document_slash_context("/bad!", 5).is_none());

        let headings = document_slash_commands("head");
        assert_eq!(headings.len(), 3);
        assert!(
            headings
                .iter()
                .all(|command| matches!(command.command, MarkdownCommand::Heading(_)))
        );
        let code = document_slash_commands("source block");
        assert_eq!(code.len(), 1);
        assert_eq!(code[0].command, MarkdownCommand::CodeBlock);
        let video = document_slash_commands("movie");
        assert_eq!(video.len(), 1);
        assert_eq!(video[0].command, MarkdownCommand::Video);
    }

    #[test]
    fn inline_mentions_insert_exact_macro_nodes_and_survive_later_projection() {
        let source = json!({"root":{
            "$":{"id":"root"},
            "type":"root",
            "version":1,
            "children":[{
                "$":{"id":"paragraph-1"},
                "type":"paragraph",
                "version":1,
                "children":[{
                    "$":{"id":"text-1"},
                    "type":"text",
                    "version":1,
                    "text":"Hello world",
                    "format":0,
                    "detail":0,
                    "mode":"normal",
                    "style":""
                }]
            }]
        }});
        let mention = MacroInlineMention::User {
            user_id: "user-1".into(),
            email: "nick@example.com".into(),
            display_name: "Nick".into(),
        };
        let (inserted, caret) = insert_inline_mention(&source, 6, &mention).unwrap();
        assert_eq!(lexical_state_to_markdown(&inserted), "Hello @Nick world");
        assert_eq!(caret, 12);
        let children = inserted["root"]["children"][0]["children"]
            .as_array()
            .unwrap();
        assert_eq!(children[0]["$"]["id"], "text-1");
        assert_eq!(children[1]["type"], "user-mention");
        assert_eq!(children[1]["userId"], "user-1");
        assert_eq!(children[1]["email"], "nick@example.com");
        assert_ne!(children[3]["$"]["id"], "text-1");

        let projected = project_markdown_edits(&inserted, "Welcome @Nick to the roadmap").unwrap();
        assert_eq!(
            projected["root"]["children"][0]["children"][1]["type"],
            "user-mention"
        );
        assert_eq!(
            lexical_state_to_markdown(&projected),
            "Welcome @Nick to the roadmap"
        );
        assert_eq!(
            lexical_state_to_macro_markdown(&projected),
            concat!(
                "Welcome <m-user-mention>",
                r#"{"userId":"user-1","email":"nick@example.com","displayName":"Nick"}"#,
                "</m-user-mention> to the roadmap"
            )
        );
    }

    #[test]
    fn macro_markdown_mentions_reopen_as_typed_nodes_and_round_trip() {
        let canonical = concat!(
            "Hello <m-user-mention>",
            r#"{"userId":"user-1","email":"nick@example.com","displayName":"Nick"}"#,
            "</m-user-mention> and <m-tag>",
            r##"{"optionId":"tag-1","propertyDefinitionId":"property-1","scope":"team","name":"Launch","color":"#ff8800"}"##,
            "</m-tag>."
        );
        let state = markdown_to_lexical_state(canonical);
        assert_eq!(
            lexical_state_to_markdown(&state),
            "Hello @Nick and #Launch."
        );
        let children = state["root"]["children"][0]["children"].as_array().unwrap();
        assert_eq!(children[1]["type"], "user-mention");
        assert_eq!(children[1]["userId"], "user-1");
        assert_eq!(children[1]["email"], "nick@example.com");
        assert_eq!(children[3]["type"], "tag-mention");
        assert_eq!(children[3]["scope"], "team");
        assert_eq!(lexical_state_to_macro_markdown(&state), canonical);

        let invalid = concat!(
            "Keep <m-user-mention>",
            r#"{"userId":"user-1","email":{"html":"<script>"}}"#,
            "</m-user-mention> literal"
        );
        let invalid_state = markdown_to_lexical_state(invalid);
        assert!(lexical_state_to_markdown(&invalid_state).contains("<m-user-mention>"));
        assert!(
            !invalid_state
                .to_string()
                .contains(r#""type":"user-mention""#)
        );

        let code_literal = concat!(
            "`<m-user-mention>",
            r#"{"userId":"user-1","email":"nick@example.com"}"#,
            "</m-user-mention>`"
        );
        let code_state = markdown_to_lexical_state(code_literal);
        assert!(!code_state.to_string().contains(r#""type":"user-mention""#));
    }

    #[test]
    fn inline_mentions_support_empty_documents_and_reject_markdown_syntax_positions() {
        let empty = markdown_to_lexical_state("");
        let mention = MacroInlineMention::Tag {
            option_id: "option-1".into(),
            property_definition_id: "property-1".into(),
            scope: "team".into(),
            name: "Important".into(),
            color: Some("red".into()),
        };
        let (inserted, caret) = insert_inline_mention(&empty, 0, &mention).unwrap();
        assert_eq!(lexical_state_to_markdown(&inserted), "#Important");
        assert_eq!(caret, "#Important".len());
        assert_eq!(
            inserted["root"]["children"][0]["children"][0]["type"],
            "tag-mention"
        );

        let heading = markdown_to_lexical_state("# Heading");
        assert_eq!(
            insert_inline_mention(&heading, 0, &mention),
            Err(MentionInsertionError::SelectionDoesNotResolveToText)
        );
    }

    #[test]
    fn renders_common_lexical_blocks_and_inline_formats() {
        let state = json!({"root":{"children":[
            {"type":"heading","tag":"h2","children":[{"type":"text","text":"Plan","format":1}]},
            {"type":"paragraph","children":[
                {"type":"text","text":"Read ","format":0},
                {"type":"link","url":"https://macro.com","children":[{"type":"text","text":"Macro","format":2}]}
            ]},
            {"type":"list","listType":"check","children":[
                {"type":"listitem","checked":true,"children":[{"type":"text","text":"Ship","format":0}]}
            ]}
        ]}});
        assert_eq!(
            lexical_state_to_markdown(&state),
            "## **Plan**\n\nRead [*Macro*](https://macro.com)\n\n- [x] Ship"
        );
    }

    #[test]
    fn unknown_nodes_keep_descendant_text() {
        let state = json!({"root":{"children":[
            {"type":"future-widget","children":[{"type":"text","text":"Visible","format":0}]}
        ]}});
        assert_eq!(lexical_state_to_markdown(&state), "Visible");
    }

    #[test]
    fn unknown_nodes_without_text_render_an_explicit_lossless_fallback() {
        let state = json!({"root":{"children":[
            {"$":{"id":"future-1"},"type":"future-widget","payload":{"meaning":42}}
        ]}});
        assert_eq!(
            lexical_state_to_markdown(&state),
            "[Unsupported Macro block · future-widget]"
        );
        let projected =
            project_markdown_edits(&state, "[Unsupported Macro block · future-widget]").unwrap();
        assert_eq!(
            projected["root"]["children"][0],
            state["root"]["children"][0]
        );
    }

    #[test]
    fn renders_macro_custom_media_mentions_cards_and_code_without_raw_json() {
        let state = json!({"root":{"children":[
            {"type":"date-mention","date":"2026-08-28","displayFormat":"MMM d"},
            {"type":"tag-mention","name":"Important","optionId":"o1","propertyDefinitionId":"p1","scope":"team"},
            {"type":"document-card","documentId":"d1","documentName":"Roadmap"},
            {"type":"video","url":"https://cdn.example/video.mp4","controls":true},
            {"type":"custom-code","language":"rust","children":[{"type":"code-highlight","text":"fn main() {}","format":0}]},
            {"type":"html-render","html":"<strong>Safe projection</strong>"}
        ]}});
        assert_eq!(
            lexical_state_to_markdown(&state),
            "@MMM d\n\n#Important\n\n@Roadmap\n\n[Video](https://cdn.example/video.mp4)\n\n```rust\nfn main() {}\n```\n\n```html\n<strong>Safe projection</strong>\n```"
        );
    }

    #[test]
    fn every_registered_macro_lexical_node_has_a_native_projection_policy() {
        let unique = MACRO_LEXICAL_NODE_TYPES
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), MACRO_LEXICAL_NODE_TYPES.len());

        for kind in MACRO_LEXICAL_NODE_TYPES {
            let mut node = json!({
                "type": kind,
                "version": 1,
                "text": "Sample",
                "content": "Sample pasted content",
                "language": "rust",
                "url": "https://example.test/item",
                "src": "https://example.test/item",
                "alt": "Sample image",
                "tag": "h2",
                "listType": "bullet",
                "documentName": "Roadmap",
                "displayName": "Nick",
                "name": "Launch",
                "groupAlias": "design",
                "label": "zed-industries/zed#1",
                "displayFormat": "Sep 1, 2026",
                "equation": "e=mc^2",
                "inline": false,
                "html": "<strong>Sample</strong>",
                "status": "completed",
                "children": [{"type": "text", "text": "Sample", "format": 0}],
            });
            if *kind == "list" {
                node["children"] = json!([{
                    "type": "listitem",
                    "children": [{"type": "text", "text": "Sample", "format": 0}],
                }]);
            } else if *kind == "table" {
                node["children"] = json!([{
                    "type": "tablerow",
                    "children": [{
                        "type": "tablecell",
                        "children": [{"type": "text", "text": "Sample", "format": 0}],
                    }],
                }]);
            }

            let rendered = render_node(&node, RenderContext::Block);
            if matches!(*kind, "agent-context" | "await") {
                assert!(rendered.is_empty(), "private node {kind} must stay hidden");
            } else {
                assert!(!rendered.is_empty(), "registered node {kind} disappeared");
                assert!(
                    !rendered.starts_with("[Unsupported Macro"),
                    "registered node {kind} used the future-node fallback: {rendered}"
                );
                assert!(
                    !rendered.contains(r#"\"type\":"#),
                    "registered node {kind} leaked serialized JSON: {rendered}"
                );
            }
        }
    }

    #[test]
    fn private_agent_context_stays_hidden_and_static_decorators_render_safely() {
        let private = json!({
            "$":{"id":"agent-context-1"},
            "type":"agent-context",
            "version":1,
            "text":"secret instructions that must never render"
        });
        let state = json!({"root":{"children":[
            private.clone(),
            {"type":"paragraph","children":[{"type":"text","text":"Visible only","format":0}]},
            {"type":"snapshot","documentId":"d1","documentName":"Roadmap","content":"old"},
            {"type":"watermark","content":"DRAFT"},
            {"type":"magic-chip","agentSessionId":"a1","status":"completed","promptedMessage":{}}
        ]}});
        assert_eq!(
            lexical_state_to_markdown(&state),
            "Visible only\n\n@Roadmap\n\nDRAFT\n\n[Macro agent · completed]"
        );
        assert!(!lexical_state_to_markdown(&state).contains("secret instructions"));

        let projected = project_markdown_edits(&state, "Visible after").unwrap_err();
        assert_eq!(projected.node_type, "snapshot");

        let hidden_only = json!({"root":{"children":[
            private.clone(),
            {"type":"paragraph","children":[{"type":"text","text":"Before","format":0}]}
        ]}});
        let edited = project_markdown_edits(&hidden_only, "After").unwrap();
        assert_eq!(edited["root"]["children"][0], private);
        assert_eq!(lexical_state_to_markdown(&edited), "After");
    }

    #[test]
    fn snapshot_and_watermark_payloads_survive_surrounding_inline_edits() {
        let snapshot = json!({
            "$":{"id":"snapshot-1","future":"preserve"},
            "type":"snapshot",
            "documentId":"d1",
            "documentName":"Roadmap",
            "blockName":"md",
            "content":"historic content"
        });
        let watermark = json!({
            "$":{"id":"watermark-1"},
            "type":"watermark",
            "content":"DRAFT",
            "authorId":"user-1"
        });
        let state = json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"type":"text","text":"See ","format":0},
                snapshot,
                {"type":"text","text":" in ","format":0},
                watermark,
                {"type":"text","text":" mode","format":0}
            ]
        }]}});
        let edited = project_markdown_edits(&state, "Review @Roadmap in DRAFT mode today").unwrap();
        assert_eq!(edited["root"]["children"][0]["children"][1], snapshot);
        assert_eq!(edited["root"]["children"][0]["children"][3], watermark);
        assert_eq!(
            lexical_state_to_markdown(&edited),
            "Review @Roadmap in DRAFT mode today"
        );
    }

    #[test]
    fn native_markdown_builds_macro_lexical_blocks() {
        let state = markdown_to_lexical_state(
            "# Plan\n\nRead **Macro** and `code`.\n\n- [x] Ship\n- [ ] Test\n\n```rust\nfn main() {}\n```",
        );
        let children = state["root"]["children"].as_array().unwrap();
        assert_eq!(children[0]["type"], "heading");
        assert_eq!(children[0]["tag"], "h1");
        assert_eq!(children[1]["children"][1]["format"], 1);
        assert_eq!(children[2]["type"], "list");
        assert_eq!(children[2]["listType"], "check");
        assert_eq!(children[2]["children"][0]["checked"], true);
        assert_eq!(children[3]["type"], "code");
        assert_eq!(children[3]["language"], "rust");
    }

    #[test]
    fn markdown_lexical_round_trip_covers_links_images_tables_and_math() {
        let state = markdown_to_lexical_state(
            "[Macro](https://macro.com)\n\n![Logo](https://macro.com/logo.png)\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n$e=mc^2$",
        );
        let rendered = lexical_state_to_markdown(&state);
        assert!(rendered.contains("[Macro](https://macro.com)"));
        assert!(rendered.contains("![Logo](https://macro.com/logo.png)"));
        assert!(rendered.contains("| A | B |"));
        assert!(rendered.contains("$e=mc^2$"));
    }

    #[test]
    fn front_matter_round_trips_without_becoming_a_fenced_code_block() {
        for (source, language) in [
            (
                "---\ntitle: Native Macro\ntags:\n  - gpui\n---\n\n# Roadmap",
                "macro-frontmatter-yaml",
            ),
            (
                "+++\ntitle = \"Native Macro\"\ndraft = false\n+++\n\n# Roadmap",
                "macro-frontmatter-toml",
            ),
        ] {
            let state = markdown_to_lexical_state(source);
            assert_eq!(state["root"]["children"][0]["type"], "code");
            assert_eq!(state["root"]["children"][0]["language"], language);
            assert_eq!(lexical_state_to_markdown(&state), source);
            assert!(!lexical_state_to_markdown(&state).starts_with("```"));
        }
    }

    #[test]
    fn editing_around_front_matter_preserves_its_lexical_identity() {
        let mut state = markdown_to_lexical_state("---\ntitle: Native Macro\n---\n\nInitial body");
        state["root"]["children"][0]["$"] = json!({
            "id": "frontmatter-stable",
            "peerId": 42,
        });
        let edited =
            project_markdown_edits(&state, "---\ntitle: Native Macro\n---\n\nUpdated body")
                .unwrap();
        assert_eq!(
            edited["root"]["children"][0]["$"],
            json!({"id": "frontmatter-stable", "peerId": 42})
        );
        assert_eq!(
            edited["root"]["children"][0]["language"],
            "macro-frontmatter-yaml"
        );
        assert_eq!(
            lexical_state_to_markdown(&edited),
            "---\ntitle: Native Macro\n---\n\nUpdated body"
        );
    }

    #[test]
    fn projection_preserves_root_state_ids_and_unknown_fields() {
        let source = json!({"root":{
            "$":{"id":"root","pinnedPropertyIds":["priority","owner"]},
            "type":"root",
            "version":1,
            "futureRootField":{"enabled":true},
            "children":[{
                "$":{"id":"paragraph-stable","peerId":42},
                "type":"paragraph",
                "version":1,
                "futureParagraphField":"keep",
                "children":[{
                    "$":{"id":"text-stable"},
                    "type":"text",
                    "version":1,
                    "text":"Before",
                    "format":0,
                    "detail":0,
                    "mode":"normal",
                    "style":""
                }]
            }]
        }});
        let projected = project_markdown_edits(&source, "After").unwrap();
        assert_eq!(
            projected["root"]["$"]["pinnedPropertyIds"],
            json!(["priority", "owner"])
        );
        assert_eq!(projected["root"]["futureRootField"]["enabled"], true);
        assert_eq!(
            projected["root"]["children"][0]["$"]["id"],
            "paragraph-stable"
        );
        assert_eq!(projected["root"]["children"][0]["$"]["peerId"], 42);
        assert_eq!(
            projected["root"]["children"][0]["futureParagraphField"],
            "keep"
        );
        assert_eq!(
            projected["root"]["children"][0]["children"][0]["$"]["id"],
            "text-stable"
        );
        assert_eq!(
            projected["root"]["children"][0]["children"][0]["text"],
            "After"
        );
    }

    #[test]
    fn pinned_properties_read_legacy_state_and_write_current_node_state() {
        let mut state = json!({"root":{
            "$":{"id":"root"},
            "pinnedPropertyIds":["legacy-priority"],
            "children":[]
        }});
        assert_eq!(pinned_property_ids(&state), vec!["legacy-priority"]);
        set_pinned_property_ids(&mut state, vec!["owner".into(), "status".into()]);
        assert_eq!(pinned_property_ids(&state), vec!["owner", "status"]);
        assert!(state["root"].get("pinnedPropertyIds").is_none());
        assert_eq!(state["root"]["$"]["id"], "root");
    }

    #[test]
    fn preserved_mentions_keep_their_exact_identity_payload() {
        let paragraph = json!({
            "$":{"id":"paragraph-with-mention"},
            "type":"paragraph",
            "version":1,
            "children":[
                {"type":"text","text":"Hello ","format":0},
                {
                    "$":{"id":"mention-1","future":"metadata"},
                    "type":"user-mention",
                    "userId":"user-1",
                    "email":"person@example.com",
                    "displayName":"Person",
                    "version":1
                }
            ]
        });
        let source = json!({"root":{"children":[
            paragraph,
            {"type":"paragraph","children":[{"type":"text","text":"Before","format":0}]}
        ]}});
        let projected = project_markdown_edits(&source, "Hello @Person\n\nAfter").unwrap();
        assert_eq!(
            projected["root"]["children"][0]["children"][1],
            source["root"]["children"][0]["children"][1]
        );
        assert_eq!(
            projected["root"]["children"][0]["$"]["id"],
            "paragraph-with-mention"
        );
        assert_eq!(
            projected["root"]["children"][1]["children"][0]["text"],
            "After"
        );
    }

    #[test]
    fn opaque_top_level_nodes_are_woven_back_into_their_exact_position() {
        let opaque = json!({
            "$":{"id":"future-1","vendor":{"revision":7}},
            "type":"future-widget",
            "children":[{"type":"text","text":"Visible widget","format":0}],
            "payload":{"must":"survive"}
        });
        let source = json!({"root":{"children":[
            {"type":"paragraph","children":[{"type":"text","text":"Before","format":0}]},
            opaque.clone(),
            {"type":"paragraph","children":[{"type":"text","text":"After","format":0}]}
        ]}});
        let projected =
            project_markdown_edits(&source, "Changed before\n\nVisible widget\n\nChanged after")
                .unwrap();
        assert_eq!(projected["root"]["children"][1], opaque);
        assert_eq!(
            projected["root"]["children"][0]["children"][0]["text"],
            "Changed before"
        );
        assert_eq!(
            projected["root"]["children"][2]["children"][0]["text"],
            "Changed after"
        );
    }

    #[test]
    fn deleting_or_ambiguously_moving_an_opaque_block_is_rejected() {
        let source = json!({"root":{"children":[
            {"type":"paragraph","children":[
                {"type":"future-inline","children":[
                    {"type":"text","text":"Hello @Person","format":0}
                ],"payload":{"must":"survive"}}
            ]},
            {"type":"paragraph","children":[{"type":"text","text":"Hello @Person","format":0}]}
        ]}});
        let missing = project_markdown_edits(&source, "Only ordinary text").unwrap_err();
        assert_eq!(missing.node_type, "future-inline");

        let ambiguous =
            project_markdown_edits(&source, "Hello @Person\n\nHello @Person").unwrap_err();
        assert_eq!(ambiguous.node_type, "future-inline");
    }

    #[test]
    fn surrounding_mention_text_is_editable_but_the_atomic_token_is_not() {
        let mention = json!({
            "$":{"id":"mention-1","future":"metadata"},
            "type":"user-mention",
            "displayName":"Person",
            "userId":"u1",
            "email":"person@example.com"
        });
        let source = json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"$":{"id":"text-1"},"type":"text","text":"Hello ","format":0},
                mention,
                {"$":{"id":"text-2"},"type":"text","text":"!","format":0}
            ]
        }]}});
        let projected = project_markdown_edits(&source, "Welcome 👋 @Person — today").unwrap();
        assert_eq!(
            projected["root"]["children"][0]["children"][1],
            source["root"]["children"][0]["children"][1]
        );
        assert_eq!(
            lexical_state_to_markdown(&projected),
            "Welcome 👋 @Person — today"
        );

        for markdown in ["Welcome Person — today", "Welcome @Someone — today"] {
            let error = project_markdown_edits(&source, markdown).unwrap_err();
            assert_eq!(error.block_index, 0);
            assert_eq!(error.node_type, "user-mention");
        }
    }

    #[test]
    fn document_stats_and_outline_ignore_fenced_headings() {
        let markdown = "# Plan\n\nTwo words\n\n```md\n# Not outline\n```\n\n## Ship ##";
        assert_eq!(
            document_stats(markdown),
            DocumentStats {
                words: 12,
                characters: markdown.chars().count(),
                lines: 9,
            }
        );
        assert_eq!(
            markdown_outline(markdown),
            vec![
                OutlineItem {
                    level: 1,
                    title: "Plan".into(),
                    line: 0,
                },
                OutlineItem {
                    level: 2,
                    title: "Ship".into(),
                    line: 8,
                },
            ]
        );
    }

    #[test]
    fn line_diff_preserves_order_and_classifies_changes() {
        assert_eq!(
            line_diff("one\ntwo\nthree", "one\n2\nthree\nfour"),
            vec![
                LineChange::Equal("one".into()),
                LineChange::Removed("two".into()),
                LineChange::Added("2".into()),
                LineChange::Equal("three".into()),
                LineChange::Added("four".into()),
            ]
        );
    }

    #[test]
    fn three_way_document_merge_keeps_non_overlapping_local_and_remote_edits() {
        let merged = merge_document_text(
            "# Plan\n\nFirst\n\nSecond\n",
            "# Plan\n\nFirst local\n\nSecond\n",
            "# Plan\n\nFirst\n\nSecond remote\n",
        )
        .unwrap();
        assert!(merged.contains("First local"));
        assert!(merged.contains("Second remote"));
    }

    #[test]
    fn three_way_document_merge_surfaces_overlapping_edits_for_resolution() {
        let conflict = merge_document_text("same\n", "mine\n", "theirs\n").unwrap_err();
        assert!(conflict.contains("<<<<<<<"));
        assert!(conflict.contains("mine"));
        assert!(conflict.contains("theirs"));
    }

    #[test]
    fn lexical_text_points_round_trip_through_markdown_syntax_and_unicode() {
        let state = json!({"root":{"children":[
            {"type":"heading","tag":"h2","children":[
                {"$":{"id":"heading-text"},"type":"text","text":"Résumé","format":1}
            ]},
            {"type":"list","listType":"bullet","children":[
                {"type":"listitem","children":[
                    {"$":{"id":"list-text"},"type":"text","text":"Ship it","format":0}
                ]}
            ]}
        ]}});
        let markdown = lexical_state_to_markdown(&state);
        let resume_start = markdown.find("Résumé").unwrap();
        let point = markdown_offset_to_lexical_point(&state, resume_start + "Ré".len()).unwrap();
        assert_eq!(point.node_id, "heading-text");
        assert_eq!(point.unicode_offset, 2);
        assert_eq!(
            lexical_point_to_markdown_offset(&state, &point),
            Some(resume_start + "Ré".len())
        );

        let bullet_marker = markdown.find("- ").unwrap();
        let snapped = markdown_offset_to_lexical_point(&state, bullet_marker).unwrap();
        assert_eq!(snapped.node_id, "list-text");
        assert_eq!(snapped.unicode_offset, 0);
    }

    #[test]
    fn selected_text_becomes_lossless_macro_comment_marks() {
        let state = json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"$":{"id":"text-1"},"type":"text","text":"Hello ","format":0},
                {"$":{"id":"text-2"},"type":"text","text":"wörld!","format":1}
            ]
        }]}});
        let markdown = lexical_state_to_markdown(&state);
        let start = markdown.find("lo ").unwrap();
        let end = markdown.find("rld").unwrap() + "rld".len();
        let annotated = anchor_document_comment(&state, start..end, "DISCUSSION:abc", 42).unwrap();
        assert_eq!(lexical_state_to_markdown(&annotated), markdown);
        let serialized = serde_json::to_string(&annotated).unwrap();
        assert_eq!(serialized.matches("comment-mark").count(), 2);
        assert_eq!(serialized.matches("DISCUSSION:abc").count(), 2);
        assert_eq!(serialized.matches("\"threadId\":42").count(), 2);
        assert!(anchor_document_comment(&annotated, start..end, "DISCUSSION:nested", 43).is_err());

        let removed = remove_document_comment_mark(&annotated, "DISCUSSION:abc");
        assert_eq!(lexical_state_to_markdown(&removed), markdown);
        assert!(
            !serde_json::to_string(&removed)
                .unwrap()
                .contains("comment-mark")
        );
    }

    #[test]
    fn local_comment_draft_finalizes_without_changing_text_or_node_identity() {
        let state = json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"$":{"id":"text-1"},"type":"text","text":"Review this sentence","format":0}
            ]
        }]}});
        let markdown = lexical_state_to_markdown(&state);
        let start = markdown.find("this").unwrap();
        let draft =
            anchor_document_comment_draft(&state, start..start + "this".len(), "DISCUSSION:draft")
                .unwrap();
        let draft_json = serde_json::to_string(&draft).unwrap();
        assert_eq!(lexical_state_to_markdown(&draft), markdown);
        assert!(draft_json.contains("\"isDraft\":true"));
        assert!(draft_json.contains("\"threadId\":-1"));

        let draft_wrapper_id = draft
            .pointer("/root/children/0/children/1/$/id")
            .and_then(Value::as_str)
            .unwrap()
            .to_owned();
        let finalized = finalize_document_comment_mark(&draft, "DISCUSSION:draft", 91).unwrap();
        let finalized_json = serde_json::to_string(&finalized).unwrap();
        assert_eq!(lexical_state_to_markdown(&finalized), markdown);
        assert!(finalized_json.contains("\"isDraft\":false"));
        assert!(finalized_json.contains("\"threadId\":91"));
        assert_eq!(
            finalized
                .pointer("/root/children/0/children/1/$/id")
                .and_then(Value::as_str),
            Some(draft_wrapper_id.as_str())
        );

        let discarded = remove_document_comment_mark(&draft, "DISCUSSION:draft");
        assert_eq!(lexical_state_to_markdown(&discarded), markdown);
        assert!(!contains_document_comment_marks(&discarded));
        assert!(finalize_document_comment_mark(&discarded, "DISCUSSION:draft", 91).is_none());
    }

    #[test]
    fn markdown_edits_preserve_and_transform_comment_ranges() {
        let state = json!({"root":{"children":[{
            "$":{"id":"paragraph-1"},
            "type":"paragraph",
            "children":[
                {"$":{"id":"text-1"},"type":"text","text":"Hello wörld!","format":0}
            ]
        }]}});
        let markdown = lexical_state_to_markdown(&state);
        let start = markdown.find("wörld").unwrap();
        let anchored =
            anchor_document_comment(&state, start..start + "wörld".len(), "DISCUSSION:abc", 42)
                .unwrap();

        let surrounding_edit = project_markdown_edits(&anchored, "Hi wörld!").unwrap();
        assert_eq!(lexical_state_to_markdown(&surrounding_edit), "Hi wörld!");
        assert!(contains_document_comment_marks(&surrounding_edit));
        assert!(
            serde_json::to_string(&surrounding_edit)
                .unwrap()
                .contains("DISCUSSION:abc")
        );

        let selected_edit = project_markdown_edits(&anchored, "Hello earth!").unwrap();
        assert_eq!(lexical_state_to_markdown(&selected_edit), "Hello earth!");
        let serialized = serde_json::to_string(&selected_edit).unwrap();
        assert!(serialized.contains("comment-mark"));
        assert!(serialized.contains("\"text\":\"earth\""));
        assert!(project_markdown_edits(&anchored, "Hello !").is_err());
    }

    #[test]
    fn rich_clipboard_envelope_round_trips_markdown_html_and_plain_text() {
        let markdown = "# Plan\n\n- **Ship** <unsafe>\n- Review `tests`";
        let payload = DocumentClipboardPayload::from_markdown(markdown);
        assert_eq!(payload.schema, DocumentClipboardPayload::SCHEMA);
        assert_eq!(payload.markdown, markdown);
        assert!(payload.html.contains("<h1>Plan</h1>"));
        assert!(payload.html.contains("<strong>Ship</strong>"));
        assert!(payload.html.contains("&lt;unsafe&gt;"));
        assert_eq!(payload.plain_text, "Plan\nShip <unsafe>\nReview tests");

        let metadata = serde_json::to_string(&payload).unwrap();
        assert_eq!(
            DocumentClipboardPayload::from_metadata(&metadata),
            Some(payload)
        );
        assert!(DocumentClipboardPayload::from_metadata("{\"schema\":\"other\"}").is_none());
    }

    #[test]
    fn custom_code_edits_retain_macro_node_class_and_extensions() {
        let source = json!({"root":{"children":[{
            "$":{"id":"code-1","peerId":"peer-a"},
            "type":"custom-code",
            "language":"rust",
            "futureSyntaxTheme":"macro-night",
            "children":[{
                "$":{"id":"code-text-1"},
                "type":"code-highlight",
                "highlightType":"keyword",
                "text":"fn old() {}",
                "format":0
            }]
        }]}});
        assert_eq!(
            lexical_state_to_markdown(&source),
            "```rust\nfn old() {}\n```"
        );

        let projected = project_markdown_edits(&source, "```rust\nfn new() {}\n```").unwrap();
        let code = &projected["root"]["children"][0];
        assert_eq!(code["type"], "custom-code");
        assert_eq!(code["futureSyntaxTheme"], "macro-night");
        assert_eq!(code["$"]["id"], "code-1");
        assert_eq!(code["children"][0]["text"], "fn new() {}\n");
        assert_eq!(code["children"][0]["$"]["id"], "code-text-1");
    }

    #[test]
    fn video_url_edits_preserve_media_identity_dimensions_and_controls() {
        let source = json!({"root":{"children":[{
            "$":{"id":"video-node-1"},
            "type":"video",
            "srcType":"url",
            "id":"media-42",
            "url":"https://cdn.example/old.mp4",
            "width":1920,
            "height":1080,
            "constrainedWidth":640,
            "constrainedHeight":360,
            "scale":0.5,
            "controls":false,
            "futurePlaybackMode":"ambient"
        }]}});
        assert_eq!(
            lexical_state_to_markdown(&source),
            "[Video](https://cdn.example/old.mp4)"
        );

        let projected =
            project_markdown_edits(&source, "[Video](https://cdn.example/new.mp4)").unwrap();
        let video = &projected["root"]["children"][0];
        assert_eq!(video["type"], "video");
        assert_eq!(video["url"], "https://cdn.example/new.mp4");
        assert_eq!(video["id"], "media-42");
        assert_eq!(video["width"], 1920);
        assert_eq!(video["height"], 1080);
        assert_eq!(video["constrainedWidth"], 640);
        assert_eq!(video["controls"], false);
        assert_eq!(video["futurePlaybackMode"], "ambient");
        assert_eq!(video["$"]["id"], "video-node-1");
    }
}

impl Focusable for NativeDocumentEditor {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        if self.rich_inline_block_id.is_some() {
            self.rich_inline_editor.read(cx).focus_handle(cx)
        } else if self.preview_enabled {
            self.preview_focus_handle.clone()
        } else {
            self.editor.read(cx).focus_handle(cx)
        }
    }
}

impl EventEmitter<DocumentEditorEvent> for NativeDocumentEditor {}

impl Render for NativeDocumentEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.preview_enabled {
            let read_only = self.read_only;
            let document_label = format!("{} document preview", self.name);
            let blocks_empty = self.rich_blocks.is_empty();
            return gpui::div()
                .id("native-document-rich-blocks")
                .key_context("NativeDocumentRichPreview")
                .role(Role::Document)
                .aria_label(document_label)
                .track_focus(&self.preview_focus_handle)
                .on_key_down(cx.listener(Self::rich_preview_key_down))
                .size_full()
                .child(
                    gpui::div()
                        .id("native-document-rich-block-list")
                        .size_full()
                        .role(Role::List)
                        .aria_label("Document blocks")
                        .when(blocks_empty, |this| {
                            this.child(
                                gpui::div()
                                    .id("native-document-rich-empty")
                                    .w_full()
                                    .max_w(px(840.0))
                                    .mx_auto()
                                    .px_6()
                                    .py_6()
                                    .opacity(0.55)
                                    .child(if read_only {
                                        "This document is empty."
                                    } else {
                                        "Start writing…"
                                    }),
                            )
                        })
                        .when(!blocks_empty, |this| {
                            this.child(
                                list(
                                    self.rich_block_list.clone(),
                                    cx.processor(Self::render_rich_block),
                                )
                                .size_full()
                                .px_6()
                                .py_6(),
                            )
                        }),
                )
                .into_any_element();
        }
        gpui::div()
            .id("native-document-source")
            .on_key_down(cx.listener(Self::source_key_down))
            .size_full()
            .child(self.editor.clone())
            .into_any_element()
    }
}
