//! Small pieces the search screen and setup share: the pill tag on a row, the bar buttons with their key, colors read
//! from the theme.
//!
//! Sizes here are pixels, not rem steps: the window is drawn to the design canvas ("Omafind UI redesign") in exact
//! pixels, the way Omarchy's own panels are, and its type follows Omarchy's monospace font at those sizes.

use gpui_kit::base::{Button, ColorTokens, StyledExt as _, Theme};
use gpui_kit::{
    App, ClickEvent, Div, ElementId, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

/// The semantic colors the window paints with: Omarchy's palette as `theme::Palette::apply` maps it.
pub fn colors(cx: &App) -> ColorTokens {
    Theme::global(cx).tokens.colors
}

/// The theme's general corner radius (Hyprland's rounding, capped).
pub fn radius(cx: &App) -> gpui_kit::Pixels {
    Theme::global(cx).tokens.radius.md
}

/// A color at `alpha` (0..1); the palette's colors are opaque, so this is the alpha.
pub fn alpha(color: Hsla, alpha: f32) -> Hsla {
    color.opacity(alpha.clamp(0.0, 1.0))
}

pub fn id(name: impl Into<SharedString>) -> ElementId {
    ElementId::Name(name.into())
}

/// A small rounded tag: why a row matched, how many like it were folded in.
pub fn pill(text: impl Into<SharedString>, strong: bool, cx: &App) -> Div {
    let c = colors(cx);
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(3.))
        .text_size(px(10.))
        .bg(if strong { c.muted } else { c.background })
        .border_1()
        .border_color(if strong { c.muted } else { c.border })
        .text_color(if strong {
            c.foreground
        } else {
            c.muted_foreground
        })
        .child(text.into())
}

/// How a bar button is drawn: the decision's default (Enter), a destructive choice, or an ordinary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Danger,
    Plain,
}

/// A button with its key shown in a keycap, as in the search bar and the setup bars: `Open ⏎`, `Back esc`.
///
/// It never takes focus: the keyboard stays with the field or the screen, which already answer the key on the cap.
/// A disabled one is drawn faint and ignores clicks.
pub struct BarButton {
    name: SharedString,
    label: SharedString,
    keys: SharedString,
    kind: ButtonKind,
    enabled: bool,
    icon: Option<&'static str>,
}

impl BarButton {
    pub fn new(
        name: impl Into<SharedString>,
        label: impl Into<SharedString>,
        keys: impl Into<SharedString>,
    ) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            keys: keys.into(),
            kind: ButtonKind::Plain,
            enabled: true,
            icon: None,
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn render(
        self,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cx: &App,
    ) -> impl IntoElement {
        let Self {
            name,
            label,
            keys,
            kind,
            enabled,
            icon,
        } = self;
        let c = colors(cx);
        let primary = kind == ButtonKind::Primary && enabled;
        let danger = kind == ButtonKind::Danger;
        let fg = if !enabled {
            alpha(c.muted_foreground, 0.6)
        } else if primary {
            c.primary_foreground
        } else if danger {
            c.destructive
        } else {
            c.foreground
        };
        let edge = if danger {
            alpha(c.destructive, 0.5)
        } else {
            c.border
        };
        let keycap = div()
            .h(px(16.))
            .min_w(px(16.))
            .px(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .text_size(px(10.))
            .bg(if primary {
                alpha(c.primary_foreground, 0.18)
            } else {
                c.background
            })
            .when(!primary, |d| d.border_1().border_color(edge))
            .text_color(if primary {
                c.primary_foreground
            } else if danger {
                alpha(c.destructive, 0.8)
            } else {
                alpha(c.muted_foreground, if enabled { 1.0 } else { 0.6 })
            })
            .child(keys);
        let hover_bg = if primary {
            alpha(c.primary, 0.85)
        } else {
            c.muted
        };
        Button::new(id(name))
            .focusable(false)
            .tab_stop(false)
            .disabled(!enabled)
            .h(px(30.))
            .pl(px(if icon.is_some() { 10. } else { 12. }))
            .pr(px(7.))
            .gap(px(if icon.is_some() { 7. } else { 8. }))
            .justify_start()
            .rounded(px(5.))
            .text_size(px(12.))
            .bg(if primary {
                c.primary
            } else if enabled {
                c.background
            } else {
                c.surface
            })
            .when(!primary, |b| b.border_1().border_color(edge))
            .text_color(fg)
            .when(kind == ButtonKind::Primary, |b| b.font_semibold())
            .when(enabled, |b| b.hover(move |style| style.bg(hover_bg)))
            .on_click(on_click)
            .when_some(icon, |b, icon| {
                b.child(
                    div()
                        .text_size(px(12.))
                        .text_color(c.muted_foreground)
                        .child(icon),
                )
            })
            .child(div().child(label))
            .child(keycap)
    }
}

/// A clickable region that is not a command button: a row, a tile, a recent search. Pointer only; the keyboard moves
/// the selection instead.
pub fn clickable(name: impl Into<SharedString>) -> gpui_kit::Stateful<Div> {
    div().id(id(name))
}
