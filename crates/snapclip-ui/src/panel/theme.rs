/// Shared colors used by the screenshot panel and the save panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelTheme {
    pub surface: [f32; 4],
    pub fill: [f32; 4],
    pub line: [f32; 4],
    pub line_soft: [f32; 4],
    pub accent: [f32; 4],
    pub accent_fill: [f32; 4],
    pub muted: [f32; 4],
    pub danger: [f32; 4],
    pub warning: [f32; 4],
}

impl PanelTheme {
    pub const DARK_OVERLAY: Self = Self {
        surface: [0.0, 0.0, 0.0, 0.72],
        fill: [1.0, 1.0, 1.0, 0.08],
        line: [1.0, 1.0, 1.0, 0.30],
        line_soft: [1.0, 1.0, 1.0, 0.18],
        accent: [31.0 / 255.0, 117.0 / 255.0, 219.0 / 255.0, 1.0],
        accent_fill: [31.0 / 255.0, 117.0 / 255.0, 219.0 / 255.0, 1.0],
        muted: [232.0 / 255.0, 236.0 / 255.0, 242.0 / 255.0, 0.62],
        danger: [1.0, 123.0 / 255.0, 123.0 / 255.0, 1.0],
        warning: [1.0, 0.70, 0.20, 1.0],
    };
}
