use gpui::{App, Global, Hsla, WindowAppearance};
use theme::{ActiveTheme, GlobalTheme, ThemeRegistry};

#[derive(Debug, Clone, Copy, Default)]
struct MacroAccessibilityStyle {
    high_contrast: bool,
}

impl Global for MacroAccessibilityStyle {}

pub fn set_high_contrast(enabled: bool, cx: &mut App) {
    cx.set_global(MacroAccessibilityStyle {
        high_contrast: enabled,
    });
    cx.refresh_windows();
}

pub fn high_contrast(cx: &App) -> bool {
    cx.try_global::<MacroAccessibilityStyle>()
        .is_some_and(|style| style.high_contrast)
}

/// Application-level semantic colors sourced directly from Zed's active theme.
///
/// This adapter keeps the Macro GPUI views independent of any particular Zed
/// theme while ensuring every surface, interaction, border, and status color is
/// driven by Zed's theme registry rather than a second application palette.
#[derive(Debug, Clone, PartialEq)]
pub struct MacroColors {
    pub background: Hsla,
    pub surface: Hsla,
    pub panel: Hsla,
    pub ink: Hsla,
    pub ink_muted: Hsla,
    pub ink_subtle: Hsla,
    pub ink_disabled: Hsla,
    pub ink_placeholder: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub border_variant: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub failure: Hsla,
    /// Foreground accent for links, icons, focus borders, and selected text.
    pub accent: Hsla,
    /// Filled accent surface matching Zed's tinted accent buttons.
    pub accent_background: Hsla,
    /// Foreground for tinted/filled controls. Zed's tinted buttons deliberately
    /// use the active theme's primary text rather than the window background;
    /// the latter has insufficient contrast on One Light's info surface.
    pub filled_control_foreground: Hsla,
    pub accent_muted: Hsla,
    pub icon_muted: Hsla,
}

impl MacroColors {
    pub fn from_zed(cx: &App) -> Self {
        let theme = cx.theme();
        let colors = theme.colors();
        let status = theme.status();

        let mut mapped = Self {
            background: colors.background,
            surface: colors.surface_background,
            panel: colors.panel_background,
            ink: colors.text,
            ink_muted: colors.text_muted,
            ink_subtle: colors.text_placeholder,
            ink_disabled: colors.text_disabled,
            ink_placeholder: colors.text_placeholder,
            hover: colors.ghost_element_hover,
            selected: colors.ghost_element_selected,
            border: colors.border,
            border_variant: colors.border_variant,
            success: status.success,
            warning: status.warning,
            failure: status.error,
            accent: colors.text_accent,
            // Matches Zed's `ButtonStyle::Tinted(TintColor::Accent)` surface.
            accent_background: status.info_background,
            filled_control_foreground: colors.text,
            accent_muted: colors.element_selected,
            icon_muted: colors.icon_muted,
        };
        if high_contrast(cx) {
            // Stay within Zed's active theme while replacing its intentionally
            // low-emphasis tokens with the nearest readable semantic tokens.
            // This affects every Macro surface because all of them consume the
            // adapter instead of maintaining feature-local palettes.
            mapped.ink_muted = colors.text;
            mapped.ink_subtle = colors.text_muted;
            mapped.ink_disabled = colors.text_muted;
            mapped.ink_placeholder = colors.text_muted;
            mapped.border = colors.text_muted;
            mapped.border_variant = colors.border;
            mapped.icon_muted = colors.icon;
        }
        mapped
    }
}

/// Select Zed's One Light or One Dark theme to match the native OS appearance.
/// Returns `false` only if the bundled theme has not been registered.
pub fn sync_zed_theme_with_system(appearance: WindowAppearance, cx: &mut App) -> bool {
    let theme_name = zed_theme_name(appearance);

    apply_zed_theme(theme_name, cx)
}

/// Apply a named theme from Zed's active theme registry.
pub fn apply_zed_theme(theme_name: &str, cx: &mut App) -> bool {
    let registry = ThemeRegistry::global(cx);
    let Ok(theme) = registry.get(theme_name) else {
        return false;
    };

    GlobalTheme::update_theme(cx, theme);
    cx.refresh_windows();
    true
}

fn zed_theme_name(appearance: WindowAppearance) -> &'static str {
    match appearance {
        WindowAppearance::Light | WindowAppearance::VibrantLight => "One Light",
        WindowAppearance::Dark | WindowAppearance::VibrantDark => theme::DEFAULT_DARK_THEME,
    }
}

/// Pick a stable entity accent from the active Zed theme's accent rotation.
pub fn entity_color(entity_type: &str, cx: &App) -> Hsla {
    let index = match entity_type {
        "task" | "project" => 3,
        "email" | "email_thread" => 1,
        "document" | "note" => 0,
        "code" | "skill" => 5,
        "chat" | "channel" => 8,
        "call" | "call_record" => 4,
        "crm_company" | "crm_contact" => 7,
        "calendar_event" => 11,
        "reminder" => 9,
        _ => return cx.theme().colors().icon_muted,
    };

    cx.theme().accents().color_for_index(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_appearance_selects_the_matching_zed_theme() {
        assert_eq!(zed_theme_name(WindowAppearance::Light), "One Light");
        assert_eq!(zed_theme_name(WindowAppearance::VibrantLight), "One Light");
        assert_eq!(
            zed_theme_name(WindowAppearance::Dark),
            theme::DEFAULT_DARK_THEME
        );
        assert_eq!(
            zed_theme_name(WindowAppearance::VibrantDark),
            theme::DEFAULT_DARK_THEME
        );
    }
}
