use crate::tui::prompts::PromptKeybind;

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum UpdateCheckConsentPrompt {
    #[strum(props(keybind = "y"))]
    Yes,
    #[strum(props(keybind = "n"))]
    #[strum(serialize = "No, don't ask again")]
    Never,
    #[strum(props(keybind = "a"))]
    #[strum(serialize = "Ask again later")]
    AskAgainLater,
}

impl PromptKeybind for UpdateCheckConsentPrompt {}

#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum UpdateBeginPrompt {
    #[cfg(feature = "self-replace")]
    #[strum(props(keybind = "d"))]
    #[strum(serialize = "Download and Install")]
    DownloadAndInstall,
    #[strum(props(keybind = "o"))]
    OpenGithubRepo,
    #[strum(props(keybind = "a"))]
    #[strum(serialize = "Ask again later")]
    AskAgainLater,
    #[strum(props(keybind = "s"))]
    SkipVersion,
}

impl PromptKeybind for UpdateBeginPrompt {}

// Windows-only, since Unix OSes let you just swap out the currently running executable.
// Meanwhile Windows requires you open a new *console window* and spawn the executable in there.
// And since I don't want to check if we're being run in CMD, just offer the option to open that or just close so the user can relaunch.
#[cfg(windows)]
#[derive(
    Debug, Clone, strum::VariantNames, strum::VariantArray, strum::EnumProperty, int_enum::IntEnum,
)]
#[repr(u8)]
#[strum(serialize_all = "title_case")]
pub enum UpdateLaunchPrompt {
    #[strum(props(keybind = "o"))]
    #[strum(serialize = "Open in a new window")]
    OpenInNewWindow,
    #[strum(props(keybind = "c"))]
    Close,
}

#[cfg(windows)]
impl PromptKeybind for UpdateLaunchPrompt {}
