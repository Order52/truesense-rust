//! The edit state of the per-power-mode lighting bindings (Changed, Undo).
//!
//! A part of [`super`] (the Lighting page's view-model); it shares the parent's names.
#![forbid(unsafe_code)]

use super::*;

// ---- per power mode bindings --------------------------------------------------------------------

/// The five power modes, in the order of the strip.
pub const MODES: [&str; 5] = crate::limits::POWER_MODES;

/// What one power mode has: the saved look and curve (read-only here) and the look being edited.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ModeEntry {
    /// The look the daemon holds for the mode.
    pub saved: Option<String>,
    /// The fan curve bound to the mode (shown, never edited here: the Cooling page owns it).
    pub curve: Option<String>,
    /// The look as the person has chosen it, saved or not.
    pub draft: Option<String>,
    /// Earlier drafts, newest last, for Undo.
    pub steps: Vec<Option<String>>,
}

impl ModeEntry {
    /// The draft differs from what is saved.
    pub fn changed(&self) -> bool {
        self.draft != self.saved
    }
}

/// The edit state of the "Lighting for \[mode\]" strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bindings {
    /// The power mode being edited.
    pub selected: String,
    /// The power mode the laptop is in now.
    pub current: Option<String>,
    /// One entry per mode.
    pub modes: BTreeMap<String, ModeEntry>,
}

impl Bindings {
    /// Reads the answer of `mode bindings`. Drafts that the person changed survive a reload
    /// (`previous`); drafts nobody touched follow what the daemon says now.
    pub fn load(result: &Value, previous: Option<&Self>) -> Self {
        let current = str_at(result, "mode").filter(|m| MODES.contains(m)).map(str::to_owned);
        let selected = previous
            .map(|p| p.selected.clone())
            .or_else(|| current.clone())
            .unwrap_or_else(|| "balanced".to_owned());
        let mut modes = BTreeMap::new();
        for mode in MODES {
            let saved = str_at(result, &format!("bindings.{mode}.look.name")).map(str::to_owned);
            let curve = str_at(result, &format!("bindings.{mode}.fan_curve.name")).map(str::to_owned);
            let old = previous.and_then(|p| p.modes.get(mode));
            let (draft, steps) = match old {
                Some(o) if o.changed() && o.draft != saved => (o.draft.clone(), o.steps.clone()),
                _ => (saved.clone(), Vec::new()),
            };
            modes.insert(mode.to_owned(), ModeEntry { saved, curve, draft, steps });
        }
        Self { selected, current, modes }
    }

    fn entry_mut(&mut self, mode: &str) -> Result<&mut ModeEntry, String> {
        self.modes.get_mut(mode).ok_or_else(|| format!("unknown power mode: {mode}"))
    }

    /// Chooses the look of `mode` (`None`: no look binding). Choosing what is already the draft is
    /// not a step.
    pub fn choose(&mut self, mode: &str, look: Option<String>) -> Result<(), String> {
        let entry = self.entry_mut(mode)?;
        if entry.draft != look {
            let before = entry.draft.take();
            entry.steps.push(before);
            entry.steps.truncate(40);
            entry.draft = look;
        }
        Ok(())
    }

    /// Takes back the last choice for `mode`; returns whether there was one.
    pub fn undo(&mut self, mode: &str) -> Result<bool, String> {
        let entry = self.entry_mut(mode)?;
        match entry.steps.pop() {
            Some(before) => {
                entry.draft = before;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Throws away every unsaved choice for `mode`.
    pub fn discard(&mut self, mode: &str) -> Result<(), String> {
        let entry = self.entry_mut(mode)?;
        entry.draft.clone_from(&entry.saved);
        entry.steps.clear();
        Ok(())
    }

    /// The bus commands that make the daemon match the draft of `mode`: bind a look, drop the look
    /// binding, or nothing when unchanged. The fan curve is never touched.
    pub fn plan(&self, mode: &str) -> Result<Vec<Value>, String> {
        let entry = self.modes.get(mode).ok_or_else(|| format!("unknown power mode: {mode}"))?;
        Ok(match (&entry.draft, &entry.saved) {
            (d, s) if d == s => Vec::new(),
            (Some(look), _) => vec![json!({"area": "mode", "verb": "bind", "args": {"mode": mode, "look": look}})],
            (None, Some(_)) => {
                vec![json!({"area": "mode", "verb": "unbind", "args": {"mode": mode, "what": "look"}})]
            }
            (None, None) => Vec::new(),
        })
    }

    /// For QML: the selection and one row per mode.
    pub fn view(&self) -> Value {
        let rows: Vec<Value> = MODES
            .iter()
            .filter_map(|mode| {
                let e = self.modes.get(*mode)?;
                Some(json!({
                    "mode": mode, "look": e.draft, "saved": e.saved, "curve": e.curve,
                    "changed": e.changed(), "can_undo": !e.steps.is_empty(), "current": self.current.as_deref() == Some(mode),
                }))
            })
            .collect();
        json!({"selected": self.selected, "current": self.current, "modes": rows})
    }
}
