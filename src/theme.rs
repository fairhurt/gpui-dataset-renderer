//! shadcn/ui "zinc" dark theme tokens, expressed as GPUI colors.

use gpui::{Hsla, hsla};

#[inline]
pub fn hsl(h: f32, s: f32, l: f32) -> Hsla {
    hsla(h / 360.0, s / 100.0, l / 100.0, 1.0)
}

#[inline]
pub fn alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

pub fn background() -> Hsla {
    hsl(240.0, 10.0, 3.9)
}
pub fn foreground() -> Hsla {
    hsl(0.0, 0.0, 98.0)
}
pub fn card() -> Hsla {
    hsl(240.0, 10.0, 3.9)
}
pub fn canvas_bg() -> Hsla {
    hsl(240.0, 8.0, 5.2)
}
pub fn muted() -> Hsla {
    hsl(240.0, 3.7, 15.9)
}
pub fn muted_foreground() -> Hsla {
    hsl(240.0, 5.0, 64.9)
}
pub fn border() -> Hsla {
    hsl(240.0, 3.7, 15.9)
}
pub fn input() -> Hsla {
    hsl(240.0, 3.7, 15.9)
}
pub fn primary() -> Hsla {
    hsl(0.0, 0.0, 98.0)
}
pub fn primary_foreground() -> Hsla {
    hsl(240.0, 5.9, 10.0)
}
pub fn ring() -> Hsla {
    hsl(240.0, 4.9, 83.9)
}

/// --chart-1 … --chart-5 (dark)
pub fn chart(i: usize) -> Hsla {
    match i % 5 {
        0 => hsl(220.0, 70.0, 50.0),
        1 => hsl(160.0, 60.0, 45.0),
        2 => hsl(30.0, 80.0, 55.0),
        3 => hsl(280.0, 65.0, 60.0),
        _ => hsl(340.0, 75.0, 55.0),
    }
}

#[cfg(target_os = "windows")]
pub const MONO: &str = "Consolas";
#[cfg(target_os = "macos")]
pub const MONO: &str = "Menlo";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const MONO: &str = "DejaVu Sans Mono";

#[cfg(target_os = "windows")]
pub const RENDERER: &str = "DirectX 11";
#[cfg(target_os = "macos")]
pub const RENDERER: &str = "Metal";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub const RENDERER: &str = "Vulkan";

pub fn destructive() -> Hsla {
    hsl(0.0, 72.0, 51.0)
}

/// Status colors (Draft, In Review, Approved, Implemented, Verified, Rejected).
pub fn status_color(ix: u8) -> Hsla {
    match ix {
        0 => hsl(240.0, 5.0, 55.0),
        1 => chart(2),
        2 => chart(0),
        3 => chart(3),
        4 => chart(1),
        5 => destructive(),
        _ => muted_foreground(),
    }
}

/// Priority colors (Critical, High, Medium, Low).
pub fn priority_color(ix: u8) -> Hsla {
    match ix {
        0 => hsl(0.0, 80.0, 62.0),
        1 => chart(2),
        2 => foreground(),
        _ => muted_foreground(),
    }
}
