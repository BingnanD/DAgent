use ratatui::style::{Color, Modifier, Style};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    pub(crate) fn binary(&self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    pub(crate) fn all() -> [Provider; 2] {
        [Provider::Claude, Provider::Codex]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ThemePreset {
    Fjord,
    Graphite,
    Solarized,
    Aurora,
    Ember,
}

impl ThemePreset {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            ThemePreset::Fjord => "fjord",
            ThemePreset::Graphite => "graphite",
            ThemePreset::Solarized => "solarized",
            ThemePreset::Aurora => "aurora",
            ThemePreset::Ember => "ember",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "fjord" | "nord" | "blue" => Some(ThemePreset::Fjord),
            "graphite" | "slate" | "gray" => Some(ThemePreset::Graphite),
            "solarized" | "sand" | "amber" => Some(ThemePreset::Solarized),
            "aurora" | "mint" | "teal" => Some(ThemePreset::Aurora),
            "ember" | "warm" | "copper" => Some(ThemePreset::Ember),
            _ => None,
        }
    }

    pub(crate) fn palette(self) -> ThemePalette {
        match self {
            ThemePreset::Fjord => ThemePalette {
                // 经典黑金主题 - 高端商务感
                prompt: Color::Rgb(192, 192, 192), // 银灰色提示符
                input_text: Color::Rgb(224, 224, 224), // 浅银灰文字
                muted_text: Color::Rgb(128, 128, 128), // 灰色次要文字
                highlight_fg: Color::Rgb(255, 255, 255), // 白色高亮前景
                highlight_bg: Color::Rgb(64, 64, 64), // 深灰高亮背景
                activity_badge_fg: Color::Rgb(255, 255, 255),
                activity_badge_bg: Color::Rgb(80, 80, 80),
                activity_text: Color::Rgb(160, 160, 160),
                status_text: Color::Rgb(140, 140, 140),
                user_fg: Color::Rgb(255, 255, 255),
                user_bg: Color::Rgb(25, 25, 25),
                claude_label: Color::Rgb(255, 127, 80), // 橙色Claude标签
                codex_label: Color::Rgb(65, 105, 225),  // 蓝色Codex标签
                processing_label: Color::Rgb(180, 180, 180),
                assistant_text: Color::Rgb(210, 210, 210),
                assistant_processing_text: Color::Rgb(170, 170, 170),
                system_text: Color::Rgb(160, 160, 160),
                tool_icon: Color::Rgb(150, 150, 150),
                tool_text: Color::Rgb(170, 170, 170),
                error_label: Color::Rgb(220, 100, 100),
                error_text: Color::Rgb(230, 120, 120),
                banner_title: Color::Rgb(200, 200, 200),
                panel_bg: Color::Rgb(10, 10, 10),
                panel_fg: Color::Rgb(210, 210, 210),
                approval_title: Color::Rgb(200, 160, 120),
                code_fg: Color::Rgb(220, 220, 220),
                code_bg: Color::Rgb(5, 5, 5),
                inline_code_fg: Color::Rgb(190, 190, 190),
                inline_code_bg: Color::Rgb(20, 20, 20),
                bullet: Color::Rgb(150, 150, 150),
            },
            ThemePreset::Graphite => ThemePalette {
                // 深海蓝主题 - 专业科技感
                prompt: Color::Rgb(100, 150, 200),     // 蓝色提示符
                input_text: Color::Rgb(180, 200, 220), // 浅蓝文字
                muted_text: Color::Rgb(80, 100, 120),  // 深蓝次要文字
                highlight_fg: Color::Rgb(200, 220, 240), // 浅蓝高亮前景
                highlight_bg: Color::Rgb(40, 60, 80),  // 中蓝高亮背景
                activity_badge_fg: Color::Rgb(200, 220, 240),
                activity_badge_bg: Color::Rgb(50, 70, 90),
                activity_text: Color::Rgb(120, 140, 160),
                status_text: Color::Rgb(90, 110, 130),
                user_fg: Color::Rgb(200, 220, 240),
                user_bg: Color::Rgb(25, 35, 45),
                claude_label: Color::Rgb(255, 127, 80), // 橙色Claude标签
                codex_label: Color::Rgb(65, 105, 225),  // 蓝色Codex标签
                processing_label: Color::Rgb(130, 160, 190),
                assistant_text: Color::Rgb(170, 190, 210),
                assistant_processing_text: Color::Rgb(140, 160, 180),
                system_text: Color::Rgb(100, 120, 140),
                tool_icon: Color::Rgb(110, 130, 150),
                tool_text: Color::Rgb(120, 140, 160),
                error_label: Color::Rgb(220, 100, 100),
                error_text: Color::Rgb(230, 120, 120),
                banner_title: Color::Rgb(150, 170, 190),
                panel_bg: Color::Rgb(10, 20, 30),
                panel_fg: Color::Rgb(170, 190, 210),
                approval_title: Color::Rgb(200, 150, 100),
                code_fg: Color::Rgb(180, 200, 220),
                code_bg: Color::Rgb(5, 15, 25),
                inline_code_fg: Color::Rgb(160, 180, 200),
                inline_code_bg: Color::Rgb(20, 30, 40),
                bullet: Color::Rgb(110, 130, 150),
            },
            ThemePreset::Solarized => ThemePalette {
                // 森林绿主题 - 自然护眼感
                prompt: Color::Rgb(120, 180, 120), // 浅绿色提示符
                input_text: Color::Rgb(180, 216, 180), // 浅绿文字
                muted_text: Color::Rgb(100, 140, 100), // 深绿次要文字
                highlight_fg: Color::Rgb(212, 240, 212), // 亮绿高亮前景
                highlight_bg: Color::Rgb(50, 80, 50), // 中绿高亮背景
                activity_badge_fg: Color::Rgb(212, 240, 212),
                activity_badge_bg: Color::Rgb(60, 90, 60),
                activity_text: Color::Rgb(140, 180, 140),
                status_text: Color::Rgb(110, 150, 110),
                user_fg: Color::Rgb(212, 240, 212),
                user_bg: Color::Rgb(25, 40, 25),
                claude_label: Color::Rgb(255, 127, 80), // 橙色Claude标签
                codex_label: Color::Rgb(65, 105, 225),  // 蓝色Codex标签
                processing_label: Color::Rgb(150, 190, 150),
                assistant_text: Color::Rgb(186, 216, 186),
                assistant_processing_text: Color::Rgb(160, 190, 160),
                system_text: Color::Rgb(120, 160, 120),
                tool_icon: Color::Rgb(130, 170, 130),
                tool_text: Color::Rgb(140, 180, 140),
                error_label: Color::Rgb(220, 120, 120),
                error_text: Color::Rgb(230, 140, 140),
                banner_title: Color::Rgb(160, 200, 160),
                panel_bg: Color::Rgb(10, 25, 10),
                panel_fg: Color::Rgb(186, 216, 186),
                approval_title: Color::Rgb(200, 170, 140),
                code_fg: Color::Rgb(190, 220, 190),
                code_bg: Color::Rgb(5, 20, 5),
                inline_code_fg: Color::Rgb(165, 200, 165),
                inline_code_bg: Color::Rgb(20, 35, 20),
                bullet: Color::Rgb(130, 170, 130),
            },
            ThemePreset::Aurora => ThemePalette {
                // 紫罗兰主题 - 创意艺术感
                prompt: Color::Rgb(216, 180, 224), // 浅紫色提示符
                input_text: Color::Rgb(240, 212, 248), // 浅紫文字
                muted_text: Color::Rgb(160, 120, 176), // 深紫次要文字
                highlight_fg: Color::Rgb(255, 255, 255), // 白色高亮前景
                highlight_bg: Color::Rgb(80, 60, 96), // 中紫高亮背景
                activity_badge_fg: Color::Rgb(255, 255, 255),
                activity_badge_bg: Color::Rgb(100, 80, 120),
                activity_text: Color::Rgb(192, 160, 208),
                status_text: Color::Rgb(176, 140, 192),
                user_fg: Color::Rgb(255, 255, 255),
                user_bg: Color::Rgb(35, 25, 45),
                claude_label: Color::Rgb(255, 127, 80), // 橙色Claude标签
                codex_label: Color::Rgb(65, 105, 225),  // 蓝色Codex标签
                processing_label: Color::Rgb(200, 160, 216),
                assistant_text: Color::Rgb(230, 200, 240),
                assistant_processing_text: Color::Rgb(200, 170, 220),
                system_text: Color::Rgb(180, 150, 200),
                tool_icon: Color::Rgb(170, 140, 190),
                tool_text: Color::Rgb(180, 150, 200),
                error_label: Color::Rgb(220, 120, 160),
                error_text: Color::Rgb(230, 140, 180),
                banner_title: Color::Rgb(210, 180, 220),
                panel_bg: Color::Rgb(20, 15, 30),
                panel_fg: Color::Rgb(230, 200, 240),
                approval_title: Color::Rgb(220, 180, 160),
                code_fg: Color::Rgb(235, 210, 245),
                code_bg: Color::Rgb(15, 10, 25),
                inline_code_fg: Color::Rgb(210, 185, 230),
                inline_code_bg: Color::Rgb(30, 20, 40),
                bullet: Color::Rgb(170, 140, 190),
            },
            ThemePreset::Ember => ThemePalette {
                // 碳纤维主题 - 现代工业感
                prompt: Color::Rgb(204, 204, 204), // 钢灰色提示符
                input_text: Color::Rgb(238, 238, 238), // 浅钢灰文字
                muted_text: Color::Rgb(153, 153, 153), // 深钢灰次要文字
                highlight_fg: Color::Rgb(255, 255, 255), // 纯白高亮前景
                highlight_bg: Color::Rgb(64, 64, 64), // 深灰高亮背景
                activity_badge_fg: Color::Rgb(255, 255, 255),
                activity_badge_bg: Color::Rgb(80, 80, 80),
                activity_text: Color::Rgb(192, 192, 192),
                status_text: Color::Rgb(170, 170, 170),
                user_fg: Color::Rgb(255, 255, 255),
                user_bg: Color::Rgb(26, 26, 26),
                claude_label: Color::Rgb(255, 127, 80), // 橙色Claude标签
                codex_label: Color::Rgb(65, 105, 225),  // 蓝色Codex标签
                processing_label: Color::Rgb(180, 180, 180),
                assistant_text: Color::Rgb(220, 220, 220),
                assistant_processing_text: Color::Rgb(190, 190, 190),
                system_text: Color::Rgb(170, 170, 170),
                tool_icon: Color::Rgb(160, 160, 160),
                tool_text: Color::Rgb(180, 180, 180),
                error_label: Color::Rgb(220, 100, 100),
                error_text: Color::Rgb(230, 120, 120),
                banner_title: Color::Rgb(210, 210, 210),
                panel_bg: Color::Rgb(12, 12, 12),
                panel_fg: Color::Rgb(220, 220, 220),
                approval_title: Color::Rgb(210, 180, 150),
                code_fg: Color::Rgb(230, 230, 230),
                code_bg: Color::Rgb(8, 8, 8),
                inline_code_fg: Color::Rgb(200, 200, 200),
                inline_code_bg: Color::Rgb(20, 20, 20),
                bullet: Color::Rgb(160, 160, 160),
            },
        }
    }
}

pub(crate) fn default_theme() -> ThemePreset {
    ThemePreset::Graphite
}

#[derive(Clone, Copy)]
pub(crate) struct ThemePalette {
    pub(crate) prompt: Color,
    pub(crate) input_text: Color,
    pub(crate) muted_text: Color,
    pub(crate) highlight_fg: Color,
    pub(crate) highlight_bg: Color,
    #[allow(dead_code)]
    pub(crate) activity_badge_fg: Color,
    #[allow(dead_code)]
    pub(crate) activity_badge_bg: Color,
    pub(crate) activity_text: Color,
    pub(crate) status_text: Color,
    pub(crate) user_fg: Color,
    pub(crate) user_bg: Color,
    pub(crate) claude_label: Color,
    pub(crate) codex_label: Color,
    pub(crate) processing_label: Color,
    pub(crate) assistant_text: Color,
    #[allow(dead_code)]
    pub(crate) assistant_processing_text: Color,
    pub(crate) system_text: Color,
    pub(crate) tool_icon: Color,
    pub(crate) tool_text: Color,
    pub(crate) error_label: Color,
    pub(crate) error_text: Color,
    pub(crate) banner_title: Color,
    pub(crate) panel_bg: Color,
    pub(crate) panel_fg: Color,
    pub(crate) approval_title: Color,
    pub(crate) code_fg: Color,
    pub(crate) code_bg: Color,
    pub(crate) inline_code_fg: Color,
    pub(crate) inline_code_bg: Color,
    pub(crate) bullet: Color,
}

impl ThemePalette {
    pub(crate) fn prompt_style(self) -> Style {
        Style::default()
            .fg(self.prompt)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn title_style(self) -> Style {
        Style::default()
            .fg(self.banner_title)
            .add_modifier(Modifier::BOLD)
    }

    pub(crate) fn body_style(self) -> Style {
        Style::default().fg(self.assistant_text)
    }

    #[allow(dead_code)]
    pub(crate) fn body_processing_style(self) -> Style {
        Style::default().fg(self.assistant_processing_text)
    }

    pub(crate) fn secondary_style(self) -> Style {
        Style::default().fg(self.system_text)
    }

    pub(crate) fn muted_style(self) -> Style {
        Style::default().fg(self.muted_text)
    }

    pub(crate) fn status_style(self) -> Style {
        Style::default().fg(self.status_text)
    }

    pub(crate) fn panel_surface_style(self) -> Style {
        Style::default().bg(self.panel_bg).fg(self.panel_fg)
    }

    pub(crate) fn panel_border_style(self) -> Style {
        Style::default().fg(self.highlight_bg)
    }

    pub(crate) fn input_surface_style(self) -> Style {
        Style::default().fg(self.input_text)
    }

    pub(crate) fn hint_selected_style(self) -> Style {
        Style::default()
            .fg(self.highlight_fg)
            .bg(self.highlight_bg)
            .add_modifier(Modifier::BOLD)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum EntryKind {
    User,
    Assistant,
    System,
    Tool,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct LogEntry {
    pub(crate) kind: EntryKind,
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) elapsed_secs: Option<u64>,
}

#[derive(Debug)]
pub(crate) enum WorkerEvent {
    Done(String),
    AgentStart(Provider),
    AgentChunk {
        provider: Provider,
        chunk: String,
    },
    AgentDone(Provider),
    Tool {
        provider: Option<Provider>,
        msg: String,
    },
    /// Progress update shown in the spinner and activity area.
    Progress {
        provider: Provider,
        msg: String,
    },
    PromotePrimary {
        to: Provider,
        reason: String,
    },
    Error(String),
}
