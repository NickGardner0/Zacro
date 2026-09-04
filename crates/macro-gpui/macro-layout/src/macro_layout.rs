use gpui::prelude::FluentBuilder;
use gpui::*;
use icons::IconName;
use macro_models::*;
use macro_ui::*;
use std::rc::Rc;

type TabSelectHandler = dyn Fn(&String, &mut Window, &mut App) + 'static;
type TabCloseHandler = dyn Fn(&String, &mut Window, &mut App) + 'static;
type FavoriteSelectHandler = dyn Fn(&SidebarItemData, &mut Window, &mut App) + 'static;
type SidebarResizeHandler = dyn Fn(&f32, &mut Window, &mut App) + 'static;
type SplitResizeHandler = dyn Fn(&f32, &mut Window, &mut App) + 'static;

#[derive(Debug, Clone)]
struct DraggedSplitHandle;

#[derive(Debug, Clone)]
struct DraggedSidebarHandle;

pub const DEFAULT_SIDEBAR_WIDTH: f32 = 220.0;
pub const MIN_SIDEBAR_WIDTH: f32 = 168.0;
pub const MAX_SIDEBAR_WIDTH: f32 = 420.0;
pub const DEFAULT_SPLIT_POSITION: f32 = 0.5;
pub const MIN_SPLIT_POSITION: f32 = 0.2;
pub const MAX_SPLIT_POSITION: f32 = 0.8;
pub const SPLIT_POSITION_STEP: f32 = 0.05;

actions!(
    macro_layout,
    [
        ToggleSidebar,
        NavigateBack,
        NavigateForward,
        NavigateToHome,
        NavigateToInbox,
        NavigateToSearch,
        NavigateToMail,
        NavigateToChannels,
        NavigateToDocuments,
        NavigateToTasks,
        NavigateToAgents,
        NavigateToCalls,
        NavigateToCRM,
        NavigateToFiles,
        NavigateToCalendar,
        NavigateToActivity,
        NavigateToReminders,
        NavigateToIntegrations,
        NavigateToSettings,
        OpenCommandPalette,
        CloseCommandPalette,
        CloseActiveTab,
        ReopenClosedTab,
        SelectNextTab,
        SelectPreviousTab,
        ToggleDetailFullscreen,
        NewDocument,
        CopyTaskBranchName,
        Quit,
        FocusNextControl,
        FocusPreviousControl,
    ]
);

// === Navigation ===

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::upper_case_acronyms)]
pub enum NavSection {
    Home,
    Inbox,
    Search,
    Mail,
    Channels,
    Documents,
    Tasks,
    Agents,
    Calls,
    CRM,
    Files,
    Calendar,
    Activity,
    Reminders,
    Integrations,
    Settings,
}

impl NavSection {
    pub fn label(&self) -> &'static str {
        match self {
            NavSection::Home => "Home",
            NavSection::Inbox => "Inbox",
            NavSection::Search => "Search",
            NavSection::Mail => "Mail",
            NavSection::Channels => "Channels",
            NavSection::Documents => "Documents",
            NavSection::Tasks => "Tasks",
            NavSection::Agents => "Agents",
            NavSection::Calls => "Calls",
            NavSection::CRM => "CRM",
            NavSection::Files => "Files",
            NavSection::Calendar => "Calendar",
            NavSection::Activity => "Activity",
            NavSection::Reminders => "Reminders",
            NavSection::Integrations => "Integrations",
            NavSection::Settings => "Settings",
        }
    }

    pub fn icon(&self) -> EntityType {
        match self {
            NavSection::Home => EntityType::ForeignEntity,
            NavSection::Inbox => EntityType::EmailThread, // reuse email icon for inbox
            NavSection::Search => EntityType::ForeignEntity,
            NavSection::Mail => EntityType::EmailThread,
            NavSection::Channels => EntityType::Channel,
            NavSection::Documents => EntityType::Document,
            NavSection::Tasks => EntityType::Document,
            NavSection::Agents => EntityType::AgentSession,
            NavSection::Calls => EntityType::Call,
            NavSection::CRM => EntityType::CrmCompany,
            NavSection::Files => EntityType::StaticFile,
            NavSection::Calendar => EntityType::CalendarEvent,
            NavSection::Activity => EntityType::ForeignEntity,
            NavSection::Reminders => EntityType::Reminder,
            NavSection::Integrations => EntityType::ForeignEntity,
            NavSection::Settings => EntityType::User,
        }
    }

    pub fn navigation_icon(&self) -> IconName {
        match self {
            NavSection::Home => IconName::ThisWindow,
            NavSection::Inbox => IconName::Archive,
            NavSection::Search => IconName::MagnifyingGlass,
            NavSection::Mail => IconName::Envelope,
            NavSection::Channels => IconName::Hash,
            NavSection::Documents => IconName::FileTextOutlined,
            NavSection::Tasks => IconName::ListTodo,
            NavSection::Agents => IconName::ZedAgent,
            NavSection::Calls => IconName::OnCall,
            NavSection::CRM => IconName::UserGroup,
            NavSection::Files => IconName::Paperclip,
            NavSection::Calendar => IconName::Clock,
            NavSection::Activity => IconName::HistoryRerun,
            NavSection::Reminders => IconName::Bell,
            NavSection::Integrations => IconName::Blocks,
            NavSection::Settings => IconName::Settings,
        }
    }

    pub fn all() -> &'static [NavSection] {
        &[
            NavSection::Home,
            NavSection::Inbox,
            NavSection::Search,
            NavSection::Mail,
            NavSection::Channels,
            NavSection::Documents,
            NavSection::Tasks,
            NavSection::Agents,
            NavSection::Calls,
            NavSection::CRM,
            NavSection::Files,
            NavSection::Calendar,
            NavSection::Activity,
            NavSection::Reminders,
            NavSection::Integrations,
            NavSection::Settings,
        ]
    }
}

// === Sidebar ===

#[derive(IntoElement)]
pub struct Sidebar {
    expanded: bool,
    width: f32,
    active_section: NavSection,
    favorites: Vec<SidebarItemData>,
    on_favorite_select: Option<Rc<FavoriteSelectHandler>>,
    on_resize: Option<Rc<SidebarResizeHandler>>,
}

#[derive(Clone)]
pub struct SidebarItemData {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub name: String,
    pub unread_count: u32,
}

impl Sidebar {
    pub fn new(active_section: NavSection) -> Self {
        Self {
            expanded: true,
            width: DEFAULT_SIDEBAR_WIDTH,
            active_section,
            favorites: Vec::new(),
            on_favorite_select: None,
            on_resize: None,
        }
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
        self
    }

    pub fn favorites(mut self, favorites: Vec<SidebarItemData>) -> Self {
        self.favorites = favorites;
        self
    }

    pub fn on_favorite_select(
        mut self,
        handler: impl Fn(&SidebarItemData, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_favorite_select = Some(Rc::new(handler));
        self
    }

    pub fn on_resize(mut self, handler: impl Fn(&f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Sidebar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let width = if self.expanded {
            px(self.width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH))
        } else {
            px(40.0)
        };

        v_flex()
            .id("macro-sidebar")
            .role(Role::Navigation)
            .aria_label("Primary navigation")
            .w(width)
            .h_full()
            .flex_none()
            .relative()
            .bg(colors.surface)
            .border_r_1()
            .border_color(colors.border)
            .overflow_hidden()
            // Team / user header
            .child(
                h_flex()
                    .id("sidebar-toggle")
                    .debug_selector(|| "SIDEBAR-TOGGLE".into())
                    .role(Role::Button)
                    .aria_label(if self.expanded {
                        "Collapse sidebar"
                    } else {
                        "Expand sidebar"
                    })
                    .aria_expanded(self.expanded)
                    .focusable()
                    .tab_stop(true)
                    .w_full()
                    .h(px(40.0))
                    .px_2()
                    .flex_none()
                    .items_center()
                    .cursor_pointer()
                    .hover(|this| this.bg(colors.hover))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(ToggleSidebar.boxed_clone(), cx);
                    })
                    .on_a11y_action(AccessibleAction::Click, |_, window, cx| {
                        window.dispatch_action(ToggleSidebar.boxed_clone(), cx);
                    })
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .w(px(20.0))
                            .h(px(20.0))
                            .rounded_md()
                            .bg(colors.accent_background)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(colors.ink)
                            .text_xs()
                            .child("M"),
                    )
                    .when(self.expanded, |this| {
                        this.child(
                            div()
                                .flex_1()
                                .ml_2()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(colors.ink)
                                .child("Macro"),
                        )
                    }),
            )
            // Navigation items
            .child(
                v_flex()
                    .flex_1()
                    .id("sidebar-nav")
                    .role(Role::List)
                    .aria_label("Workspace sections")
                    .overflow_y_scroll()
                    .p_1()
                    // Favorites section
                    .when(!self.favorites.is_empty(), |this| {
                        this.child(SectionHeader::new("Favorites")).children(
                            self.favorites.into_iter().map(|fav| {
                                let on_select = self.on_favorite_select.clone();
                                let selected_favorite = fav.clone();
                                SidebarItem::new(fav.name)
                                    .icon(fav.entity_type)
                                    .unread(fav.unread_count)
                                    .when_some(on_select, |item, handler| {
                                        item.on_click(move |window, cx| {
                                            handler(&selected_favorite, window, cx)
                                        })
                                    })
                            }),
                        )
                    })
                    // Main navigation
                    .child(SectionHeader::new("Navigation"))
                    .children(NavSection::all().iter().map(|section| {
                        let section = *section;
                        SidebarItem::new(section.label())
                            .zed_icon(section.navigation_icon())
                            .selected(section == self.active_section)
                            .on_click(move |window, cx| {
                                dispatch_navigation(section, window, cx);
                            })
                    })),
            )
            .when(self.expanded, |this| {
                let current_width = self.width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                let set_value = self.on_resize.clone();
                let increment = self.on_resize.clone();
                let decrement = self.on_resize.clone();
                let keyboard_resize = self.on_resize.clone();
                let reset = self.on_resize.clone();
                this.child(
                    div()
                        .id("macro-sidebar-resize-handle")
                        .debug_selector(|| "SIDEBAR-RESIZE-HANDLE".into())
                        .role(Role::Splitter)
                        .aria_label("Resize primary navigation")
                        .aria_numeric_value(current_width as f64)
                        .aria_min_numeric_value(MIN_SIDEBAR_WIDTH as f64)
                        .aria_max_numeric_value(MAX_SIDEBAR_WIDTH as f64)
                        .aria_numeric_value_step(16.0)
                        .focusable()
                        .tab_stop(true)
                        .absolute()
                        .right_0()
                        .top_0()
                        .w(px(8.0))
                        .h_full()
                        .cursor_col_resize()
                        .hover(|this| this.bg(colors.accent.opacity(0.35)))
                        .focus_visible(|this| this.bg(colors.accent.opacity(0.55)))
                        .on_drag(DraggedSidebarHandle, |_, _, _, cx| cx.new(|_| gpui::Empty))
                        .when_some(set_value, |this, handler| {
                            this.on_a11y_action(
                                AccessibleAction::SetValue,
                                move |data, window, cx| {
                                    if let Some(gpui::accesskit::ActionData::NumericValue(value)) =
                                        data
                                    {
                                        let width = (*value as f32)
                                            .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                                        handler(&width, window, cx);
                                    }
                                },
                            )
                        })
                        .when_some(increment, |this, handler| {
                            this.on_a11y_action(
                                AccessibleAction::Increment,
                                move |_, window, cx| {
                                    let width = (current_width + 16.0)
                                        .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                                    handler(&width, window, cx);
                                },
                            )
                        })
                        .when_some(decrement, |this, handler| {
                            this.on_a11y_action(
                                AccessibleAction::Decrement,
                                move |_, window, cx| {
                                    let width = (current_width - 16.0)
                                        .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                                    handler(&width, window, cx);
                                },
                            )
                        })
                        .when_some(keyboard_resize, |this, handler| {
                            this.on_key_down(move |event, window, cx| {
                                let width = match event.keystroke.key.as_str() {
                                    "left" => current_width - 16.0,
                                    "right" => current_width + 16.0,
                                    "home" => MIN_SIDEBAR_WIDTH,
                                    "end" => MAX_SIDEBAR_WIDTH,
                                    _ => return,
                                }
                                .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                                handler(&width, window, cx);
                                cx.stop_propagation();
                            })
                        })
                        .when_some(reset, |this, handler| {
                            this.on_click(move |event, window, cx| {
                                if event.click_count() == 2 {
                                    handler(&DEFAULT_SIDEBAR_WIDTH, window, cx);
                                    cx.stop_propagation();
                                }
                            })
                        }),
                )
            })
    }
}

// === Tab Bar ===

#[derive(Debug, Clone)]
pub struct TabData {
    pub entity_type: EntityType,
    pub entity_id: String,
    pub label: SharedString,
    pub is_active: bool,
}

#[derive(IntoElement)]
pub struct TabBar {
    tabs: Vec<TabData>,
    scroll_handle: Option<ScrollHandle>,
    on_select: Option<Rc<TabSelectHandler>>,
    on_close: Option<Rc<TabCloseHandler>>,
}

impl TabBar {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            scroll_handle: None,
            on_select: None,
            on_close: None,
        }
    }

    pub fn tabs(mut self, tabs: Vec<TabData>) -> Self {
        self.tabs = tabs;
        self
    }

    pub fn track_scroll(mut self, scroll_handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(scroll_handle.clone());
        self
    }

    pub fn on_select(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    pub fn on_close(mut self, handler: impl Fn(&String, &mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Rc::new(handler));
        self
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for TabBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        if let Some(active_index) = self.tabs.iter().position(|tab| tab.is_active)
            && let Some(scroll_handle) = &self.scroll_handle
        {
            scroll_handle.scroll_to_item(active_index);
        }

        h_flex()
            .id("macro-tab-list")
            .role(Role::TabList)
            .aria_label("Open entities")
            .w_full()
            .h(px(32.0))
            .flex_none()
            .bg(colors.surface)
            .border_b_1()
            .border_color(colors.border)
            .overflow_x_scroll()
            .when_some(self.scroll_handle, |this, scroll_handle| {
                this.track_scroll(&scroll_handle)
            })
            .children(self.tabs.into_iter().map(|tab| {
                let tab_id = tab.entity_id.clone();
                let accessibility_tab_id = tab.entity_id.clone();
                let close_tab_id = tab.entity_id.clone();
                let accessibility_close_tab_id = tab.entity_id.clone();
                let on_select = self.on_select.clone();
                let on_accessibility_select = self.on_select.clone();
                let on_close = self.on_close.clone();
                let on_accessibility_close = self.on_close.clone();
                let tab_label = tab.label.clone();
                let debug_tab_label = tab.label.clone();
                let debug_close_label = tab.label.clone();
                let bg = if tab.is_active {
                    colors.panel
                } else {
                    hsla(0., 0., 0., 0.)
                };
                let border_color = if tab.is_active {
                    colors.accent
                } else {
                    hsla(0., 0., 0., 0.)
                };
                h_flex()
                    .id(format!("entity-tab-{tab_id}"))
                    .debug_selector(move || format!("TAB-{debug_tab_label}"))
                    .role(Role::Tab)
                    .aria_label(tab_label)
                    .aria_selected(tab.is_active)
                    .focusable()
                    .tab_stop(true)
                    .h_full()
                    .min_w(px(120.0))
                    .max_w(px(260.0))
                    .flex_none()
                    .px_3()
                    .gap_2()
                    .items_center()
                    .bg(bg)
                    .border_b_2()
                    .border_color(border_color)
                    .cursor_pointer()
                    .hover(|this| this.bg(colors.hover))
                    .when_some(on_select, |this, handler| {
                        this.on_click(move |_: &ClickEvent, window, cx| {
                            handler(&tab_id, window, cx)
                        })
                    })
                    .when_some(on_accessibility_select, |this, handler| {
                        this.on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                            handler(&accessibility_tab_id, window, cx)
                        })
                    })
                    .child(EntityIcon::new(tab.entity_type).size(14.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(if tab.is_active {
                                colors.ink
                            } else {
                                colors.ink_muted
                            })
                            .truncate()
                            .child(tab.label),
                    )
                    .when_some(on_close, |this, handler| {
                        this.child(
                            div()
                                .id(format!("close-entity-tab-{close_tab_id}"))
                                .debug_selector(move || format!("TAB-CLOSE-{debug_close_label}"))
                                .role(Role::Button)
                                .aria_label("Close tab")
                                .focusable()
                                .tab_stop(true)
                                .size(px(18.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .text_xs()
                                .text_color(colors.ink_subtle)
                                .hover(|this| this.bg(colors.hover).text_color(colors.ink))
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    cx.stop_propagation();
                                    handler(&close_tab_id, window, cx)
                                })
                                .when_some(on_accessibility_close, |this, handler| {
                                    this.on_a11y_action(
                                        AccessibleAction::Click,
                                        move |_, window, cx| {
                                            handler(&accessibility_close_tab_id, window, cx)
                                        },
                                    )
                                })
                                .child("×"),
                        )
                    })
            }))
    }
}

// === Content Pane ===

#[derive(IntoElement)]
pub struct ContentPane {
    content: AnyElement,
}

impl ContentPane {
    pub fn new(content: impl IntoElement) -> Self {
        Self {
            content: content.into_any_element(),
        }
    }
}

impl RenderOnce for ContentPane {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        div()
            .id("macro-content")
            .role(Role::Main)
            .aria_label("Workspace content")
            .flex_1()
            .h_full()
            .bg(colors.background)
            .overflow_hidden()
            .child(self.content)
    }
}

// === Split Pane ===

#[derive(IntoElement)]
pub struct SplitPane {
    left: AnyElement,
    right: Option<AnyElement>,
    split_position: f32, // 0.0-1.0, percentage for left pane
    on_resize: Option<Rc<SplitResizeHandler>>,
}

impl SplitPane {
    pub fn new(left: impl IntoElement) -> Self {
        Self {
            left: left.into_any_element(),
            right: None,
            split_position: DEFAULT_SPLIT_POSITION,
            on_resize: None,
        }
    }

    pub fn right(mut self, right: impl IntoElement) -> Self {
        self.right = Some(right.into_any_element());
        self
    }

    pub fn split_position(mut self, position: f32) -> Self {
        self.split_position = position;
        self
    }

    pub fn on_resize(mut self, handler: impl Fn(&f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SplitPane {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let split_position = self
            .split_position
            .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
        match self.right {
            Some(right) => {
                let on_resize = self.on_resize;
                let drag_resize = on_resize.clone();
                let set_value = on_resize.clone();
                let increment = on_resize.clone();
                let decrement = on_resize.clone();
                let keyboard_resize = on_resize.clone();
                let reset = on_resize;
                h_flex()
                    .id("macro-split-pane")
                    .role(Role::Group)
                    .aria_label("List and detail panes")
                    .flex_1()
                    .h_full()
                    .on_drag_move::<DraggedSplitHandle>(move |event, window, cx| {
                        let width = event.bounds.right() - event.bounds.left();
                        if width <= px(0.0) {
                            return;
                        }
                        let ratio = ((event.event.position.x - event.bounds.left()) / width)
                            .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
                        if let Some(handler) = &drag_resize {
                            handler(&ratio, window, cx);
                        }
                    })
                    .child(
                        div()
                            .id("macro-split-list-pane")
                            .role(Role::Region)
                            .aria_label("Entity list")
                            .w(relative(split_position))
                            .flex_none()
                            .h_full()
                            .overflow_hidden()
                            .child(self.left),
                    )
                    .child(
                        div()
                            .id("macro-split-divider")
                            .role(Role::Splitter)
                            .aria_label("Resize list and detail panes")
                            .aria_numeric_value(split_position as f64 * 100.0)
                            .aria_min_numeric_value(MIN_SPLIT_POSITION as f64 * 100.0)
                            .aria_max_numeric_value(MAX_SPLIT_POSITION as f64 * 100.0)
                            .aria_numeric_value_step(SPLIT_POSITION_STEP as f64 * 100.0)
                            .focusable()
                            .tab_stop(true)
                            .relative()
                            .w(px(1.0))
                            .h_full()
                            .flex_none()
                            .bg(colors.border)
                            .hover(|this| this.bg(colors.accent.opacity(0.55)))
                            .focus_visible(|this| this.bg(colors.accent))
                            .when_some(set_value, |this, handler| {
                                this.on_a11y_action(
                                    AccessibleAction::SetValue,
                                    move |data, window, cx| {
                                        if let Some(gpui::accesskit::ActionData::NumericValue(
                                            value,
                                        )) = data
                                        {
                                            let ratio = (*value as f32 / 100.0)
                                                .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
                                            handler(&ratio, window, cx);
                                        }
                                    },
                                )
                            })
                            .when_some(increment, |this, handler| {
                                this.on_a11y_action(
                                    AccessibleAction::Increment,
                                    move |_, window, cx| {
                                        let ratio = (split_position + SPLIT_POSITION_STEP)
                                            .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
                                        handler(&ratio, window, cx);
                                    },
                                )
                            })
                            .when_some(decrement, |this, handler| {
                                this.on_a11y_action(
                                    AccessibleAction::Decrement,
                                    move |_, window, cx| {
                                        let ratio = (split_position - SPLIT_POSITION_STEP)
                                            .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
                                        handler(&ratio, window, cx);
                                    },
                                )
                            })
                            .when_some(keyboard_resize, |this, handler| {
                                this.on_key_down(move |event, window, cx| {
                                    let ratio = match event.keystroke.key.as_str() {
                                        "left" => split_position - SPLIT_POSITION_STEP,
                                        "right" => split_position + SPLIT_POSITION_STEP,
                                        "home" => MIN_SPLIT_POSITION,
                                        "end" => MAX_SPLIT_POSITION,
                                        _ => return,
                                    }
                                    .clamp(MIN_SPLIT_POSITION, MAX_SPLIT_POSITION);
                                    handler(&ratio, window, cx);
                                    cx.stop_propagation();
                                })
                            })
                            .when_some(reset, |this, handler| {
                                this.on_click(move |event, window, cx| {
                                    if event.click_count() == 2 {
                                        handler(&DEFAULT_SPLIT_POSITION, window, cx);
                                        cx.stop_propagation();
                                    }
                                })
                            })
                            .child(
                                div()
                                    .id("macro-split-resize-handle")
                                    .debug_selector(|| "SPLIT-HANDLE".into())
                                    .absolute()
                                    .left(px(-3.5))
                                    .w(px(8.0))
                                    .h_full()
                                    .cursor_col_resize()
                                    .on_drag(DraggedSplitHandle, |_, _, _, cx| {
                                        cx.new(|_| gpui::Empty)
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .id("macro-split-detail-pane")
                            .role(Role::Region)
                            .aria_label("Entity detail")
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .child(right),
                    )
            }
            None => div()
                .id("macro-single-pane")
                .role(Role::Main)
                .aria_label("Workspace content")
                .flex_1()
                .h_full()
                .overflow_hidden()
                .child(self.left),
        }
    }
}

// === Status Bar ===

#[derive(IntoElement)]
pub struct StatusBar {
    connection_status: ConnectionStatus,
    notification_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Connecting,
    Disconnected,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            connection_status: ConnectionStatus::Disconnected,
            notification_count: 0,
        }
    }

    pub fn connection_status(mut self, status: ConnectionStatus) -> Self {
        self.connection_status = status;
        self
    }

    pub fn notification_count(mut self, count: u32) -> Self {
        self.notification_count = count;
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for StatusBar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let status_color = match self.connection_status {
            ConnectionStatus::Connected => colors.success,
            ConnectionStatus::Connecting => colors.warning,
            ConnectionStatus::Disconnected => colors.failure,
        };

        h_flex()
            .id("macro-status-bar")
            .role(Role::Status)
            .aria_label(match self.connection_status {
                ConnectionStatus::Connected => "Connected",
                ConnectionStatus::Connecting => "Connecting",
                ConnectionStatus::Disconnected => "Disconnected",
            })
            .w_full()
            .h(px(22.0))
            .px_2()
            .flex_none()
            .items_center()
            .justify_between()
            .bg(colors.surface)
            .border_t_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(status_color))
                    .child(div().text_xs().text_color(colors.ink_subtle).child(
                        match self.connection_status {
                            ConnectionStatus::Connected => "Connected",
                            ConnectionStatus::Connecting => "Connecting...",
                            ConnectionStatus::Disconnected => "Disconnected",
                        },
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Badge::new(self.notification_count)),
            )
    }
}

pub fn dispatch_navigation(section: NavSection, window: &mut Window, cx: &mut App) {
    match section {
        NavSection::Home => window.dispatch_action(NavigateToHome.boxed_clone(), cx),
        NavSection::Inbox => window.dispatch_action(NavigateToInbox.boxed_clone(), cx),
        NavSection::Search => window.dispatch_action(NavigateToSearch.boxed_clone(), cx),
        NavSection::Mail => window.dispatch_action(NavigateToMail.boxed_clone(), cx),
        NavSection::Channels => window.dispatch_action(NavigateToChannels.boxed_clone(), cx),
        NavSection::Documents => window.dispatch_action(NavigateToDocuments.boxed_clone(), cx),
        NavSection::Tasks => window.dispatch_action(NavigateToTasks.boxed_clone(), cx),
        NavSection::Agents => window.dispatch_action(NavigateToAgents.boxed_clone(), cx),
        NavSection::Calls => window.dispatch_action(NavigateToCalls.boxed_clone(), cx),
        NavSection::CRM => window.dispatch_action(NavigateToCRM.boxed_clone(), cx),
        NavSection::Files => window.dispatch_action(NavigateToFiles.boxed_clone(), cx),
        NavSection::Calendar => window.dispatch_action(NavigateToCalendar.boxed_clone(), cx),
        NavSection::Activity => window.dispatch_action(NavigateToActivity.boxed_clone(), cx),
        NavSection::Reminders => window.dispatch_action(NavigateToReminders.boxed_clone(), cx),
        NavSection::Integrations => {
            window.dispatch_action(NavigateToIntegrations.boxed_clone(), cx)
        }
        NavSection::Settings => window.dispatch_action(NavigateToSettings.boxed_clone(), cx),
    }
}

// === Main Layout ===

#[derive(Clone)]
pub struct AppLayoutState {
    pub active_section: NavSection,
    pub sidebar_expanded: bool,
    pub sidebar_width: f32,
    pub tabs: Vec<TabData>,
    pub tab_scroll_handle: ScrollHandle,
    pub favorites: Vec<SidebarItemData>,
    pub connection_status: ConnectionStatus,
    pub notification_count: u32,
}

impl Default for AppLayoutState {
    fn default() -> Self {
        Self {
            active_section: NavSection::Inbox,
            sidebar_expanded: true,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            tabs: Vec::new(),
            tab_scroll_handle: ScrollHandle::new(),
            favorites: Vec::new(),
            connection_status: ConnectionStatus::Disconnected,
            notification_count: 0,
        }
    }
}

#[derive(IntoElement)]
pub struct AppLayout {
    state: AppLayoutState,
    content: AnyElement,
    on_tab_select: Option<Rc<TabSelectHandler>>,
    on_tab_close: Option<Rc<TabCloseHandler>>,
    on_favorite_select: Option<Rc<FavoriteSelectHandler>>,
    on_sidebar_resize: Option<Rc<SidebarResizeHandler>>,
}

impl AppLayout {
    pub fn new(state: AppLayoutState, content: impl IntoElement) -> Self {
        Self {
            state,
            content: content.into_any_element(),
            on_tab_select: None,
            on_tab_close: None,
            on_favorite_select: None,
            on_sidebar_resize: None,
        }
    }

    pub fn on_tab_select(
        mut self,
        handler: impl Fn(&String, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_select = Some(Rc::new(handler));
        self
    }

    pub fn on_tab_close(
        mut self,
        handler: impl Fn(&String, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_tab_close = Some(Rc::new(handler));
        self
    }

    pub fn on_favorite_select(
        mut self,
        handler: impl Fn(&SidebarItemData, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_favorite_select = Some(Rc::new(handler));
        self
    }

    pub fn on_sidebar_resize(
        mut self,
        handler: impl Fn(&f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_sidebar_resize = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for AppLayout {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = macro_colors(cx);
        let sidebar_resize = self.on_sidebar_resize;
        v_flex()
            .id("macro-workspace")
            .role(Role::Application)
            .aria_label("Macro")
            .w_full()
            .h_full()
            .bg(colors.background)
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .min_h_0()
                    .overflow_hidden()
                    .when_some(sidebar_resize.clone(), |this, handler| {
                        this.on_drag_move::<DraggedSidebarHandle>(move |event, window, cx| {
                            let width = (event.event.position.x - event.bounds.left())
                                .as_f32()
                                .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                            handler(&width, window, cx);
                        })
                    })
                    .child(
                        Sidebar::new(self.state.active_section)
                            .expanded(self.state.sidebar_expanded)
                            .width(self.state.sidebar_width)
                            .favorites(self.state.favorites)
                            .when_some(self.on_favorite_select, |sidebar, handler| {
                                sidebar.on_favorite_select(move |favorite, window, cx| {
                                    handler(favorite, window, cx)
                                })
                            })
                            .when_some(sidebar_resize, |sidebar, handler| {
                                sidebar
                                    .on_resize(move |width, window, cx| handler(width, window, cx))
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .h_full()
                            .min_w_0()
                            .min_h_0()
                            .overflow_hidden()
                            .child(
                                TabBar::new()
                                    .tabs(self.state.tabs)
                                    .track_scroll(&self.state.tab_scroll_handle)
                                    .when_some(self.on_tab_select, |tabs, handler| {
                                        tabs.on_select(move |tab_id, window, cx| {
                                            handler(tab_id, window, cx)
                                        })
                                    })
                                    .when_some(self.on_tab_close, |tabs, handler| {
                                        tabs.on_close(move |tab_id, window, cx| {
                                            handler(tab_id, window, cx)
                                        })
                                    }),
                            )
                            .child(self.content),
                    ),
            )
            .child(
                StatusBar::new()
                    .connection_status(self.state.connection_status)
                    .notification_count(self.state.notification_count),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AppLayout, AppLayoutState, ConnectionStatus, ContentPane, DEFAULT_SIDEBAR_WIDTH,
        NavSection, NavigateToTasks, SplitPane, TabData, ToggleSidebar,
    };
    use gpui::{
        Context, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding, Modifiers,
        MouseButton, ParentElement as _, Render, ScrollHandle, TestAppContext, Window, div, point,
        px,
    };
    use macro_models::EntityType;
    use std::{cell::RefCell, rc::Rc};

    struct LayoutHarness {
        state: AppLayoutState,
        focus: FocusHandle,
        selected_tabs: Rc<RefCell<Vec<String>>>,
        closed_tabs: Rc<RefCell<Vec<String>>>,
        resized_sidebar_widths: Rc<RefCell<Vec<f32>>>,
        resized_split_positions: Rc<RefCell<Vec<f32>>>,
    }

    impl LayoutHarness {
        fn new(
            window: &mut Window,
            cx: &mut Context<Self>,
            selected_tabs: Rc<RefCell<Vec<String>>>,
            closed_tabs: Rc<RefCell<Vec<String>>>,
            resized_sidebar_widths: Rc<RefCell<Vec<f32>>>,
            resized_split_positions: Rc<RefCell<Vec<f32>>>,
        ) -> Self {
            let focus = cx.focus_handle();
            window.focus(&focus, cx);
            Self {
                state: AppLayoutState {
                    active_section: NavSection::Inbox,
                    sidebar_expanded: true,
                    sidebar_width: DEFAULT_SIDEBAR_WIDTH,
                    tabs: vec![TabData {
                        entity_type: EntityType::Document,
                        entity_id: "document-1".into(),
                        label: "Roadmap".into(),
                        is_active: true,
                    }],
                    tab_scroll_handle: ScrollHandle::new(),
                    favorites: vec![],
                    connection_status: ConnectionStatus::Connected,
                    notification_count: 2,
                },
                focus,
                selected_tabs,
                closed_tabs,
                resized_sidebar_widths,
                resized_split_positions,
            }
        }
    }

    impl Render for LayoutHarness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let selected_tabs = self.selected_tabs.clone();
            let closed_tabs = self.closed_tabs.clone();
            let resized_sidebar_widths = self.resized_sidebar_widths.clone();
            let resized_split_positions = self.resized_split_positions.clone();
            div()
                .track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &NavigateToTasks, _, cx| {
                    this.state.active_section = NavSection::Tasks;
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                    this.state.sidebar_expanded = !this.state.sidebar_expanded;
                    cx.notify();
                }))
                .child(
                    AppLayout::new(
                        self.state.clone(),
                        ContentPane::new(
                            SplitPane::new(div().child("List"))
                                .right(div().child("Detail"))
                                .split_position(0.42)
                                .on_resize(move |ratio, _, _| {
                                    resized_split_positions.borrow_mut().push(*ratio);
                                }),
                        ),
                    )
                    .on_tab_select(move |id, _, _| {
                        selected_tabs.borrow_mut().push(id.clone());
                    })
                    .on_tab_close(move |id, _, _| {
                        closed_tabs.borrow_mut().push(id.clone());
                    })
                    .on_sidebar_resize(move |width, _, _| {
                        resized_sidebar_widths.borrow_mut().push(*width);
                    }),
                )
        }
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            cx.bind_keys([
                KeyBinding::new("cmd-b", ToggleSidebar, None),
                KeyBinding::new("cmd-5", NavigateToTasks, None),
            ]);
        });
    }

    #[gpui::test]
    fn shell_pointer_controls_route_navigation_tabs_and_sidebar(cx: &mut TestAppContext) {
        init_test(cx);
        let selected_tabs = Rc::new(RefCell::new(Vec::new()));
        let closed_tabs = Rc::new(RefCell::new(Vec::new()));
        let resized_sidebar_widths = Rc::new(RefCell::new(Vec::new()));
        let resized_split_positions = Rc::new(RefCell::new(Vec::new()));
        let (view, cx) = cx.add_window_view({
            let selected_tabs = selected_tabs.clone();
            let closed_tabs = closed_tabs.clone();
            let resized_sidebar_widths = resized_sidebar_widths.clone();
            let resized_split_positions = resized_split_positions.clone();
            move |window, cx| {
                LayoutHarness::new(
                    window,
                    cx,
                    selected_tabs,
                    closed_tabs,
                    resized_sidebar_widths,
                    resized_split_positions,
                )
            }
        });

        let task_bounds = cx
            .debug_bounds("SIDEBAR-Tasks")
            .expect("Tasks navigation item should be painted");
        cx.simulate_click(task_bounds.center(), Modifiers::default());
        assert_eq!(
            view.read_with(cx, |view, _| view.state.active_section),
            NavSection::Tasks
        );

        let toggle_bounds = cx
            .debug_bounds("SIDEBAR-TOGGLE")
            .expect("sidebar toggle should be painted");
        cx.simulate_click(toggle_bounds.center(), Modifiers::default());
        assert!(!view.read_with(cx, |view, _| view.state.sidebar_expanded));
        cx.simulate_keystrokes("cmd-b");
        assert!(
            view.read_with(cx, |view, _| view.state.sidebar_expanded),
            "the shell shortcut should dispatch through the focused GPUI tree"
        );

        view.update(cx, |view, cx| {
            view.state.active_section = NavSection::Inbox;
            cx.notify();
        });
        cx.simulate_keystrokes("cmd-5");
        assert_eq!(
            view.read_with(cx, |view, _| view.state.active_section),
            NavSection::Tasks
        );

        let tab_bounds = cx
            .debug_bounds("TAB-Roadmap")
            .expect("entity tab should be painted");
        cx.simulate_click(tab_bounds.center(), Modifiers::default());
        assert_eq!(&*selected_tabs.borrow(), &["document-1"]);

        let close_bounds = cx
            .debug_bounds("TAB-CLOSE-Roadmap")
            .expect("tab close button should be painted");
        cx.simulate_click(close_bounds.center(), Modifiers::default());
        assert_eq!(&*closed_tabs.borrow(), &["document-1"]);
        assert_eq!(
            selected_tabs.borrow().len(),
            1,
            "close click must not bubble into tab selection"
        );

        let tab_scroll_handle = view.read_with(cx, |view, _| view.state.tab_scroll_handle.clone());
        view.update(cx, |view, cx| {
            view.state.tabs = (0..16)
                .map(|index| TabData {
                    entity_type: EntityType::Document,
                    entity_id: format!("document-{index}"),
                    label: format!("Workspace document {index}").into(),
                    is_active: index == 15,
                })
                .collect();
            cx.notify();
        });
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        assert!(
            tab_scroll_handle.max_offset().x > px(0.0),
            "overflowing Zed tabs should have horizontal scroll range"
        );
        assert!(
            tab_scroll_handle.offset().x < px(0.0),
            "the active overflow tab should be revealed automatically"
        );

        let sidebar_handle = cx
            .debug_bounds("SIDEBAR-RESIZE-HANDLE")
            .expect("expanded sidebar should expose a resize handle");
        let start = sidebar_handle.center();
        let target = point(px(312.0), start.y);
        cx.simulate_mouse_move(start, None, Modifiers::default());
        cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_move(
            point(start.x + px(12.0), start.y),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(target, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, Modifiers::default());
        let observed_width = resized_sidebar_widths
            .borrow()
            .last()
            .copied()
            .expect("sidebar drag should emit a width");
        assert!((observed_width - 312.0).abs() < 1.0, "{observed_width}");

        let (sidebar_splitter_id, content_splitter_id) = cx.update(|window, cx| {
            window.debug_set_a11y_active(true);
            window.refresh();
            window.draw(cx).clear(cx);
            let tree: serde_json::Value = serde_json::from_str(
                &window
                    .debug_a11y_tree_json()
                    .expect("active accessibility should retain the painted tree"),
            )
            .unwrap();
            let nodes = tree["nodes"].as_object().unwrap();
            let splitter_id = |label: &str| {
                let splitter = nodes
                    .values()
                    .find(|node| node["aria"]["label"] == label)
                    .unwrap_or_else(|| panic!("{label} should be exposed to AccessKit"));
                assert_eq!(splitter["aria"]["role"], "Splitter");
                let actions = splitter["aria"]["on_action"].as_array().unwrap();
                for required in ["Focus", "SetValue", "Increment", "Decrement"] {
                    assert!(
                        actions.iter().any(|action| action == required),
                        "{label} missing {required}"
                    );
                }
                splitter["accesskit_id"]
                    .as_str()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap()
            };
            (
                splitter_id("Resize primary navigation"),
                splitter_id("Resize list and detail panes"),
            )
        });
        resized_sidebar_widths.borrow_mut().clear();
        cx.update(|window, cx| {
            window.debug_perform_a11y_action(
                gpui::accesskit::ActionRequest {
                    action: gpui::AccessibleAction::Increment,
                    target_tree: gpui::accesskit::TreeId::ROOT,
                    target_node: gpui::accesskit::NodeId(sidebar_splitter_id),
                    data: None,
                },
                cx,
            );
        });
        assert_eq!(&*resized_sidebar_widths.borrow(), &[236.0]);

        resized_split_positions.borrow_mut().clear();
        cx.update(|window, cx| {
            window.debug_perform_a11y_action(
                gpui::accesskit::ActionRequest {
                    action: gpui::AccessibleAction::Increment,
                    target_tree: gpui::accesskit::TreeId::ROOT,
                    target_node: gpui::accesskit::NodeId(content_splitter_id),
                    data: None,
                },
                cx,
            );
        });
        let observed_ratio = resized_split_positions
            .borrow()
            .last()
            .copied()
            .expect("content splitter increment should emit a ratio");
        assert!((observed_ratio - 0.47).abs() < f32::EPSILON);

        assert!(cx.debug_bounds("SPLIT-HANDLE").is_some());
    }
}
