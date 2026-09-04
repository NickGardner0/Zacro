use gpui::prelude::FluentBuilder;
use gpui::*;
use icons::IconName;
use macro_models::EntityType;
use macro_theme::MacroColors;
use std::rc::Rc;
use ui::{
    Avatar as ZedAvatar, Color, Headline, HeadlineSize, Label, LabelCommon as _, LabelSize,
    ListItem, Toggleable as _,
};

type ActivateHandler = dyn Fn(&mut Window, &mut App) + 'static;

pub fn h_flex() -> Div {
    div().flex().flex_row().items_center()
}

pub fn v_flex() -> Div {
    div().flex().flex_col()
}

pub fn macro_colors(cx: &App) -> MacroColors {
    MacroColors::from_zed(cx)
}

fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        h: color.h,
        s: color.s,
        l: color.l,
        a: alpha,
    }
}

// === Entity Icon ===

#[derive(IntoElement)]
pub struct EntityIcon {
    entity_type: EntityType,
    size: f32,
    color: Option<Hsla>,
}

impl EntityIcon {
    pub fn new(entity_type: EntityType) -> Self {
        Self {
            entity_type,
            size: 16.0,
            color: None,
        }
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for EntityIcon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let color = self.color.unwrap_or(colors.icon_muted);
        let icon = match self.entity_type {
            EntityType::Document | EntityType::Skill => IconName::FileTextOutlined,
            EntityType::Chat => IconName::Chat,
            EntityType::Channel => IconName::Hash,
            EntityType::ChannelMessage => IconName::QueueMessage,
            EntityType::EmailThread | EntityType::EmailMessage => IconName::Envelope,
            EntityType::Project => IconName::Folder,
            EntityType::Call | EntityType::CallRecord => IconName::OnCall,
            EntityType::CalendarEvent => IconName::Clock,
            EntityType::CrmCompany => IconName::UserGroup,
            EntityType::CrmContact | EntityType::User => IconName::Person,
            EntityType::Reminder => IconName::Bell,
            EntityType::AgentSession => IconName::ZedAgent,
            EntityType::Team => IconName::UserGroup,
            EntityType::StaticFile => IconName::Paperclip,
            EntityType::ForeignEntity => IconName::Link,
        };
        svg()
            .w(px(self.size))
            .h(px(self.size))
            .flex_none()
            .path(icon.path())
            .text_color(color)
    }
}

// === Entity Badge ===

#[derive(IntoElement)]
pub struct EntityBadge {
    entity_type: EntityType,
    label: SharedString,
}

impl EntityBadge {
    pub fn new(entity_type: EntityType, label: impl Into<SharedString>) -> Self {
        Self {
            entity_type,
            label: label.into(),
        }
    }
}

impl RenderOnce for EntityBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let color = macro_theme::entity_color(&self.entity_type.to_string(), cx);
        let label: SharedString = match self.label.as_ref() {
            "emailThread" | "email_thread" => "Email".into(),
            "callRecord" | "call_record" => "Call".into(),
            "agentSession" | "agent_session" => "Agent".into(),
            "staticFile" | "static_file" => "File".into(),
            "crmCompany" | "crm_company" => "Company".into(),
            "crmContact" | "crm_contact" => "Contact".into(),
            "channelMessage" | "channel_message" => "Message".into(),
            "calendarEvent" | "calendar_event" => "Event".into(),
            "task" => "Task".into(),
            "document" => "Document".into(),
            _ => self.label,
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_0p5()
            .rounded_full()
            .border_1()
            .border_color(with_alpha(color, 0.24))
            .bg(with_alpha(color, 0.07))
            .child(EntityIcon::new(self.entity_type).size(12.0).color(color))
            .child(div().text_xs().text_color(colors.ink_muted).child(label))
    }
}

// === Avatar ===

#[derive(IntoElement)]
pub struct Avatar {
    name: SharedString,
    url: Option<SharedString>,
    size: f32,
}

impl Avatar {
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            url: None,
            size: 32.0,
        }
    }

    pub fn url(mut self, url: impl Into<SharedString>) -> Self {
        self.url = Some(url.into());
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let debug_name = self.name.clone();
        if let Some(url) = self.url {
            return div()
                .debug_selector(move || format!("AVATAR-{debug_name}"))
                .child(ZedAvatar::new(url).size(px(self.size)))
                .into_any_element();
        }

        let initials: String = self
            .name
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .take(2)
            .collect();
        let colors = macro_colors(cx);
        div()
            .debug_selector(move || format!("AVATAR-{debug_name}"))
            .flex()
            .items_center()
            .justify_center()
            .w(px(self.size))
            .h(px(self.size))
            .rounded_full()
            .bg(colors.accent_muted)
            .text_color(colors.ink)
            .text_sm()
            .child(initials)
            .into_any_element()
    }
}

// === Badge (notification count) ===

#[derive(IntoElement)]
pub struct Badge {
    count: u32,
}

impl Badge {
    pub fn new(count: u32) -> Self {
        Self { count }
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.count == 0 {
            return div().into_any_element();
        }
        let colors = macro_colors(cx);
        div()
            .flex()
            .items_center()
            .justify_center()
            .min_w(px(18.0))
            .h(px(18.0))
            .px_1()
            .rounded_full()
            .bg(colors.failure)
            .text_color(rgb(0xffffff))
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .child(format!("{}", self.count))
            .into_any_element()
    }
}

// === Sidebar Item ===

#[derive(IntoElement)]
pub struct SidebarItem {
    label: SharedString,
    icon: Option<EntityType>,
    zed_icon: Option<IconName>,
    selected: bool,
    unread_count: u32,
    on_click: Option<Rc<ActivateHandler>>,
}

impl SidebarItem {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            zed_icon: None,
            selected: false,
            unread_count: 0,
            on_click: None,
        }
    }

    pub fn icon(mut self, entity_type: EntityType) -> Self {
        self.icon = Some(entity_type);
        self.zed_icon = None;
        self
    }

    pub fn zed_icon(mut self, icon: IconName) -> Self {
        self.zed_icon = Some(icon);
        self.icon = None;
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub fn unread(mut self, count: u32) -> Self {
        self.unread_count = count;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SidebarItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let text_color = if self.selected {
            colors.ink
        } else {
            colors.ink_muted
        };
        let label = self.label.clone();
        let debug_label = self.label.clone();
        let click_handler = self.on_click;

        div()
            .debug_selector(move || format!("SIDEBAR-{debug_label}"))
            .w_full()
            .child(
                ListItem::new(self.label.to_string())
                    .aria_role(Role::Button)
                    .aria_label(label)
                    .tab_index(0)
                    .toggle_state(self.selected)
                    .rounded()
                    .height(px(28.0))
                    .when_some(self.icon, |this, entity_type| {
                        this.start_slot(EntityIcon::new(entity_type).size(14.0).color(text_color))
                    })
                    .when_some(self.zed_icon, |this, icon| {
                        this.start_slot(
                            svg()
                                .size(px(14.0))
                                .path(icon.path())
                                .text_color(text_color),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(text_color)
                            .truncate()
                            .child(self.label),
                    )
                    .when(self.unread_count > 0, |this| {
                        this.end_slot(Badge::new(self.unread_count))
                    })
                    .when_some(click_handler, |this, handler| {
                        this.on_click(move |_, window, cx| handler(window, cx))
                    }),
            )
    }
}

// === Section Header ===

#[derive(IntoElement)]
pub struct SectionHeader {
    label: SharedString,
}

impl SectionHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for SectionHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let accessible_label = self.label.to_string();
        div()
            .id(format!("section-header-{accessible_label}"))
            .role(Role::Heading)
            .aria_level(2)
            .aria_label(accessible_label)
            .w_full()
            .px_2()
            .pt_3()
            .pb_1()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.ink_subtle)
            .child(self.label.to_uppercase())
    }
}

// === Empty State ===

#[derive(IntoElement)]
pub struct EmptyState {
    title: SharedString,
    description: Option<SharedString>,
    icon: Option<IconName>,
}

// === Product composition primitives ===

/// Compact, Zed-style header used by list and detail panes.
///
/// Macro previously rebuilt every header from raw `div()` chains, which made
/// title baselines, toolbar heights, and border treatment drift by feature.
/// Keeping the shell here gives every feature the same visual rhythm while
/// leaving feature-specific actions outside the service-agnostic UI crate.
#[derive(IntoElement)]
pub struct PaneHeader {
    title: SharedString,
    description: Option<SharedString>,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
}

impl PaneHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            start_slot: None,
            end_slot: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn start_slot(mut self, slot: impl IntoElement) -> Self {
        self.start_slot = Some(slot.into_any_element());
        self
    }

    pub fn end_slot(mut self, slot: impl IntoElement) -> Self {
        self.end_slot = Some(slot.into_any_element());
        self
    }
}

impl RenderOnce for PaneHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        h_flex()
            .w_full()
            .min_h(px(40.0))
            .px_3()
            .gap_2()
            .flex_none()
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.surface)
            .when_some(self.start_slot, |this, slot| this.child(slot))
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(
                        Label::new(self.title)
                            .size(LabelSize::Default)
                            .single_line(),
                    )
                    .when_some(self.description, |this, description| {
                        this.child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .single_line(),
                        )
                    }),
            )
            .when_some(self.end_slot, |this, slot| this.child(slot))
    }
}

/// Shared horizontal action rail. Feature code supplies Zed buttons and menus;
/// this component supplies the stable sizing and surface treatment.
#[derive(IntoElement)]
pub struct Toolbar {
    compact: bool,
    children: Vec<AnyElement>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            compact: false,
            children: Vec::new(),
        }
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for Toolbar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for Toolbar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        h_flex()
            .w_full()
            .min_h(if self.compact { px(32.0) } else { px(40.0) })
            .px_2()
            .gap_1()
            .flex_none()
            .border_b_1()
            .border_color(colors.border_variant)
            .bg(colors.surface)
            .children(self.children)
    }
}

/// Header for a modal surface, using the same semantic typography as Zed's
/// native modals while allowing the caller to supply an explicit close button.
#[derive(IntoElement)]
pub struct DialogHeader {
    title: SharedString,
    description: Option<SharedString>,
    icon: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl DialogHeader {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            icon: None,
            trailing: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for DialogHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        h_flex()
            .w_full()
            .min_h(px(48.0))
            .px_3()
            .py_2()
            .gap_2()
            .flex_none()
            .border_b_1()
            .border_color(colors.border_variant)
            .when_some(self.icon, |this, icon| this.child(icon))
            .child(
                v_flex()
                    .min_w_0()
                    .flex_1()
                    .child(Headline::new(self.title).size(HeadlineSize::XSmall))
                    .when_some(self.description, |this, description| {
                        this.child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .single_line(),
                        )
                    }),
            )
            .when_some(self.trailing, |this, trailing| this.child(trailing))
    }
}

/// Visually groups related fields without turning every row into a bordered
/// table. This mirrors Zed's restrained settings and modal sections.
#[derive(IntoElement)]
pub struct FormSection {
    title: Option<SharedString>,
    description: Option<SharedString>,
    contained: bool,
    children: Vec<AnyElement>,
}

impl FormSection {
    pub fn new() -> Self {
        Self {
            title: None,
            description: None,
            contained: false,
            children: Vec::new(),
        }
    }

    pub fn titled(title: impl Into<SharedString>) -> Self {
        Self::new().title(title)
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn contained(mut self, contained: bool) -> Self {
        self.contained = contained;
        self
    }
}

impl Default for FormSection {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for FormSection {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for FormSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        v_flex()
            .w_full()
            .gap_2()
            .when(self.contained, |this| {
                this.p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(colors.border_variant)
                    .bg(colors.surface)
            })
            .when_some(self.title, |this, title| {
                this.child(
                    Label::new(title)
                        .size(LabelSize::Small)
                        .weight(FontWeight::SEMIBOLD),
                )
            })
            .when_some(self.description, |this, description| {
                this.child(
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .children(self.children)
    }
}

/// Semantic form-field wrapper with optional helper text. Controls remain
/// caller-owned so fields can contain a Zed editor, dropdown, picker, or toggle.
#[derive(IntoElement)]
pub struct FormField {
    label: SharedString,
    description: Option<SharedString>,
    required: bool,
    children: Vec<AnyElement>,
}

impl FormField {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            description: None,
            required: false,
            children: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }
}

impl ParentElement for FormField {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for FormField {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        v_flex()
            .min_w_0()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Label::new(self.label)
                            .size(LabelSize::Small)
                            .weight(FontWeight::MEDIUM),
                    )
                    .when(self.required, |this| {
                        this.child(
                            Label::new("Required")
                                .size(LabelSize::XSmall)
                                .color(Color::Muted),
                        )
                    }),
            )
            .children(self.children)
            .when_some(self.description, |this, description| {
                this.child(
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
    }
}

/// Consistent visual frame for native Zed editor-backed fields.
#[derive(IntoElement)]
pub struct InputFrame {
    active: bool,
    multiline: bool,
    invalid: bool,
    children: Vec<AnyElement>,
}

impl InputFrame {
    pub fn new() -> Self {
        Self {
            active: false,
            multiline: false,
            invalid: false,
            children: Vec::new(),
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn multiline(mut self, multiline: bool) -> Self {
        self.multiline = multiline;
        self
    }

    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }
}

impl Default for InputFrame {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for InputFrame {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for InputFrame {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        div()
            .w_full()
            .min_h(if self.multiline { px(112.0) } else { px(32.0) })
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if self.invalid {
                colors.failure
            } else if self.active {
                colors.accent
            } else {
                colors.border_variant
            })
            .bg(colors.background)
            .children(self.children)
    }
}

/// Two-column property layout used by task, CRM, document, and sharing forms.
#[derive(IntoElement)]
pub struct PropertyGrid {
    columns: u16,
    children: Vec<AnyElement>,
}

impl PropertyGrid {
    pub fn new() -> Self {
        Self {
            columns: 2,
            children: Vec::new(),
        }
    }

    pub fn columns(mut self, columns: u16) -> Self {
        self.columns = columns.max(1);
        self
    }
}

impl Default for PropertyGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl ParentElement for PropertyGrid {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for PropertyGrid {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div()
            .grid()
            .grid_cols(self.columns)
            .gap_3()
            .w_full()
            .children(self.children)
    }
}

impl EmptyState {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            description: None,
            icon: None,
        }
    }

    pub fn description(mut self, desc: impl Into<SharedString>) -> Self {
        self.description = Some(desc.into());
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let accessible_title = self.title.clone();
        v_flex()
            .id(format!("empty-state-{accessible_title}"))
            .role(Role::Status)
            .aria_label(accessible_title)
            .items_center()
            .justify_center()
            .w_full()
            .h_full()
            .gap_3()
            .when_some(self.icon, |this, icon| {
                this.child(
                    div()
                        .size(px(48.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(colors.border_variant)
                        .bg(colors.surface)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .size(px(20.0))
                                .path(icon.path())
                                .text_color(colors.icon_muted),
                        ),
                )
            })
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(colors.ink_muted)
                    .child(self.title),
            )
            .when_some(self.description, |this, desc| {
                this.child(div().text_sm().text_color(colors.ink_subtle).child(desc))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{Avatar, EmptyState, EntityBadge, SectionHeader, SidebarItem};
    use gpui::{
        Context, IntoElement, Modifiers, ParentElement, Render, Styled, TestAppContext, Window, div,
    };
    use macro_models::EntityType;
    use std::{cell::Cell, rc::Rc};

    struct UiHarness {
        activations: Rc<Cell<u32>>,
    }

    impl Render for UiHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let activations = self.activations.clone();
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(
                    SidebarItem::new("Inbox")
                        .icon(EntityType::EmailThread)
                        .selected(true)
                        .unread(7)
                        .on_click(move |_, _| activations.set(activations.get() + 1)),
                )
                .child(SectionHeader::new("Navigation"))
                .child(EntityBadge::new(EntityType::Document, "document"))
                .child(Avatar::new("Ada Lovelace").size(24.0))
                .child(Avatar::new("Remote User").url("https://example.com/avatar.png"))
                .child(EmptyState::new("Nothing here").description("Try another view"))
        }
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    #[gpui::test]
    fn core_components_render_and_sidebar_routes_pointer_activation(cx: &mut TestAppContext) {
        init_test(cx);
        let activations = Rc::new(Cell::new(0));
        let (_view, cx) = cx.add_window_view({
            let activations = activations.clone();
            move |_, _| UiHarness { activations }
        });

        let sidebar_bounds = cx
            .debug_bounds("SIDEBAR-Inbox")
            .expect("the Zed list-item-backed sidebar entry should be painted");
        cx.simulate_click(sidebar_bounds.center(), Modifiers::default());
        assert_eq!(activations.get(), 1);

        assert!(cx.debug_bounds("AVATAR-Ada Lovelace").is_some());
        assert!(cx.debug_bounds("AVATAR-Remote User").is_some());
    }
}
