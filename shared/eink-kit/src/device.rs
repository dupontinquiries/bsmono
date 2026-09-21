//! Which tablet we are running on, from the tablet's side.
//!
//! `EINK_MODEL` wins when set (gitmind's `eink_fleet` passes it at launch,
//! and the AppLoad manifest it installs carries it). Otherwise the device
//! tree model string is matched the same way `cockpit::eink_fleet` matches
//! it from the host. Those strings are unverified on hardware; a miss falls
//! back to an architecture guess and says so in the log.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Model {
    Rm1,
    Rm2,
    PaperPro,
    PaperProMove,
    PaperPure,
}

impl Model {
    pub fn parse(s: &str) -> Option<Model> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rm1" => Some(Model::Rm1),
            "rm2" => Some(Model::Rm2),
            "rmpp" | "ferrari" | "paper-pro" => Some(Model::PaperPro),
            "rmppm" | "rmppmove" | "chiappa" | "paper-pro-move" => Some(Model::PaperProMove),
            "rmppure" | "tatsu" | "paper-pure" => Some(Model::PaperPure),
            _ => None,
        }
    }
    pub fn from_device_tree(text: &str) -> Option<Model> {
        let l = text.to_ascii_lowercase();
        if l.contains("ferrari") {
            Some(Model::PaperPro)
        } else if l.contains("chiappa") {
            Some(Model::PaperProMove)
        } else if l.contains("tatsu") {
            Some(Model::PaperPure)
        } else if l.contains("remarkable 2") {
            Some(Model::Rm2)
        } else if l.contains("remarkable 1") || l.contains("prototype 1") {
            Some(Model::Rm1)
        } else {
            None
        }
    }
    /// Panel size, portrait. Same values as qtfb's common.h.
    pub fn size(self) -> (u32, u32) {
        match self {
            Model::Rm1 | Model::Rm2 | Model::PaperPure => (1404, 1872),
            Model::PaperPro => (1620, 2160),
            Model::PaperProMove => (954, 1696),
        }
    }
    pub fn color(self) -> bool {
        matches!(self, Model::PaperPro | Model::PaperProMove)
    }
    /// qtfb framebuffer type for its RGB565 mode on this model.
    pub fn qtfb_rgb565_type(self) -> u8 {
        match self {
            Model::Rm1 | Model::Rm2 => 0, // FBFMT_RM2FB (RGB16, rM2 size)
            Model::PaperPro => 3,         // FBFMT_RMPP_RGB565
            Model::PaperProMove => 6,     // FBFMT_RMPPM_RGB565
            Model::PaperPure => 9,        // FBFMT_RMPPURE_RGB565
        }
    }
}

/// Detect, with a human-readable note on how.
pub fn detect() -> (Model, String) {
    if let Ok(s) = std::env::var("EINK_MODEL") {
        if let Some(m) = Model::parse(&s) {
            return (m, format!("EINK_MODEL={s}"));
        }
    }
    let dt = std::fs::read_to_string("/proc/device-tree/model").unwrap_or_default();
    if let Some(m) = Model::from_device_tree(&dt) {
        return (m, format!("device tree {:?}", dt.trim_end_matches('\0').trim()));
    }
    let guess = if cfg!(target_arch = "aarch64") { Model::PaperPro } else { Model::Rm2 };
    (guess, format!("GUESS from architecture (device tree said {:?}); set EINK_MODEL", dt.trim_end_matches('\0').trim()))
}
