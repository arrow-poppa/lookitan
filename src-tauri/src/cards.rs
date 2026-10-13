// Saved words. Each card is a JSON object owned by the front end (word, translation, definition,
// context, IPA, CEFR, source window, Anki note id, ...); this module only stores them and their
// screenshot files.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use base64::Engine;
use parking_lot::Mutex;
use serde_json::{json, Value};

pub struct CardStore {
    path: PathBuf,
    media_dir: PathBuf,
    cards: Mutex<Vec<Value>>,
}

impl CardStore {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("words.json");
        let cards = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<Value>>(&b).ok())
            .unwrap_or_default();
        Self { path, media_dir: data_dir.join("media"), cards: Mutex::new(cards) }
    }

    pub fn media_dir(&self) -> &Path {
        &self.media_dir
    }

    fn persist(&self, cards: &[Value]) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(cards)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<Value> {
        self.cards.lock().clone()
    }

    /// Inserts or replaces (by `id`) a card. A `imageData` field (base64 PNG) is moved to a file
    /// and replaced by `image` (the file name).
    pub fn save(&self, mut card: Value) -> Result<Value> {
        let obj = card.as_object_mut().ok_or_else(|| anyhow!("a card must be an object"))?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let id = obj.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("w{now:x}"));
        obj.insert("id".into(), json!(id));
        obj.entry("createdAt").or_insert(json!(now as u64));
        obj.insert("updatedAt".into(), json!(now as u64));
        if let Some(data) = obj.remove("imageData").and_then(|v| v.as_str().map(str::to_string)) {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim_start_matches("data:image/png;base64,"))?;
            std::fs::create_dir_all(&self.media_dir)?;
            let name = format!("{id}.png");
            std::fs::write(self.media_dir.join(&name), bytes)?;
            obj.insert("image".into(), json!(name));
        }
        let mut cards = self.cards.lock();
        match cards.iter().position(|c| c.get("id").and_then(Value::as_str) == Some(id.as_str())) {
            Some(i) => cards[i] = card.clone(),
            None => cards.insert(0, card.clone()),
        }
        self.persist(&cards)?;
        Ok(card)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let mut cards = self.cards.lock();
        cards.retain(|c| c.get("id").and_then(Value::as_str) != Some(id));
        let _ = std::fs::remove_file(self.media_dir.join(format!("{id}.png")));
        self.persist(&cards)
    }

    pub fn media(&self, name: &str) -> Option<Vec<u8>> {
        if name.contains(['/', '\\']) || name.contains("..") {
            return None;
        }
        std::fs::read(self.media_dir.join(name)).ok()
    }
}
