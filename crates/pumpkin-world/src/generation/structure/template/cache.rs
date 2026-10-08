//! Template caching for embedded structure templates.
//!
//! This module provides a lazy-loading cache for structure templates that are
//! embedded in the binary at compile time using `include_bytes!`.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use dashmap::DashMap;
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use pumpkin_nbt::Nbt;
use pumpkin_nbt::nbt_compress::read_gzip_compound_tag;
use pumpkin_util::identifier::Identifier;

use super::{StructureTemplate, structure_template::TemplateError};

/// Vanilla's implicit namespace.
const DEFAULT_NAMESPACE: &str = "minecraft";

/// Canonicalizes a resource id to fully-qualified `namespace:path` form.
///
/// A bare `foo` becomes `minecraft:foo`, matching vanilla resolution.
fn canonicalize(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("{DEFAULT_NAMESPACE}:{name}")
    }
}

/// A cache for loaded structure templates.
///
/// Templates are loaded lazily on first access and stored for reuse.
/// Keys are fully-qualified resource ids, so `foo` and `minecraft:foo`
/// share a single entry.
/// The cache is thread-safe and can be accessed from multiple threads.
pub struct TemplateCache {
    cache: DashMap<String, Arc<StructureTemplate>>,
    dynamic_templates: DashMap<String, Arc<[u8]>>,
}

impl Default for TemplateCache {
    fn default() -> Self {
        Self::new()
    }
}

impl TemplateCache {
    /// Creates a new empty template cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
            dynamic_templates: DashMap::new(),
        }
    }

    /// Registers raw template NBT bytes from a dynamic datapack.
    pub fn register_template(&self, name: &str, bytes: Arc<[u8]>) {
        let key = canonicalize(name);
        self.dynamic_templates.insert(key.clone(), bytes);
        self.cache.remove(&key);
    }

    /// Clears all dynamically registered templates (e.g. during datapack reload).
    pub fn clear_dynamic(&self) {
        self.dynamic_templates.clear();
        self.cache.clear();
    }

    /// Gets a template by `name`, loading it from embedded resources if not cached.
    ///
    /// `name` may be bare (`foo`) or namespaced (`minecraft:foo`, `pumpkin:foo`).
    ///
    /// Returns the loaded template wrapped in an `Arc`, or `None` if the template
    /// doesn't exist or failed to load.
    pub fn get(&self, name: &str) -> Option<Arc<StructureTemplate>> {
        match self.get_or_error(name) {
            Ok(template) => Some(template),
            Err(TemplateError::MissingField("template file not found")) => None,
            Err(e) => {
                tracing::error!("Failed to load template '{}': {}", name, e);
                None
            }
        }
    }

    /// Gets a template by name, returning an error if loading fails.
    ///
    /// # Errors
    ///
    /// Returns an error if the template doesn't exist or fails to parse.
    pub fn get_or_error(&self, name: &str) -> Result<Arc<StructureTemplate>, TemplateError> {
        let key = canonicalize(name);

        // Check cache first
        if let Some(template) = self.cache.get(&key) {
            return Ok(Arc::clone(&template));
        }

        let mut template = if let Some(bytes) = self.dynamic_templates.get(&key) {
            StructureTemplate::from_nbt_bytes(&bytes)?
        } else if let Some(static_template) =
            pumpkin_data::structure_template::get_structure_template(&key)
        {
            StructureTemplate::from_static(static_template)?
        } else {
            return Err(TemplateError::MissingField("template file not found"));
        };
        template.name = Some(key.clone());

        let arc = Arc::new(template);
        self.cache.insert(key, Arc::clone(&arc));
        Ok(arc)
    }

    /// Preloads a list of templates into the cache.
    ///
    /// This can be useful during server startup to avoid loading delays
    /// during gameplay.
    pub fn preload(&self, names: &[&str]) {
        for name in names {
            if let Err(e) = self.get_or_error(name) {
                tracing::warn!("Failed to preload template '{}': {}", name, e);
            }
        }
    }

    /// Returns the number of cached templates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Returns whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    /// Clears all cached templates.
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// Vanilla `StructureTemplateManager#getOrCreate` + repository put: puts a
    /// fully built template into the running cache under `name`.
    pub fn store(&self, name: &str, mut template: StructureTemplate) {
        let key = canonicalize(name);
        template.name = Some(key.clone());
        self.cache.insert(key, Arc::new(template));
    }

    /// Vanilla `StructureTemplateManager#save`: writes a template as compressed
    /// NBT below the world's `generated` directory
    /// (`generated/<namespace>/structure/<path>.nbt`) and keeps it in the
    /// running cache, mirrored after the filled template stays in the vanilla
    /// template repository even before the file write.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is not a valid identifier, its path can
    /// not be turned into a portable file name, or the file write fails.
    pub fn save(
        &self,
        name: &str,
        template: &StructureTemplate,
        generated_dir: &Path,
    ) -> Result<(), TemplateSaveError> {
        let identifier =
            Identifier::parse(name).map_err(|_| TemplateSaveError::InvalidName(name.to_owned()))?;

        let file = template_file(generated_dir, &identifier)?;
        self.store(&canonicalize(name), template.clone());
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_template_compressed(&file, template)?;

        Ok(())
    }

    /// Vanilla `StructureTemplateManager#remove`: drops the in-memory entry;
    /// the file on disk stays.
    pub fn remove(&self, name: &str) {
        self.cache.remove(&canonicalize(name));
    }

    /// Vanilla `DirectoryTemplateSource`: reads a world-saved template from
    /// `generated/<namespace>/structure/<path>.nbt`. Accepts both the vanilla
    /// zlib encoding and Pumpkin's gzip encoding of the same payload.
    #[must_use]
    pub fn load_from_disk(&self, file: &Path) -> Option<StructureTemplate> {
        let bytes = std::fs::read(file).ok()?;
        let compound = if bytes.starts_with(gzip_magic()) {
            read_gzip_compound_tag(std::io::Cursor::new(bytes.as_slice())).ok()?
        } else {
            let mut buf = Vec::new();
            ZlibDecoder::new(std::io::Cursor::new(bytes.as_slice()))
                .read_to_end(&mut buf)
                .ok()?;
            let mut reader = pumpkin_nbt::deserializer::NbtReadHelperJava::new(
                std::io::Cursor::new(buf.as_slice()),
            );
            pumpkin_nbt::Nbt::read(&mut reader).ok()?.root_tag
        };
        StructureTemplate::from_nbt_compound(&compound).ok()
    }
}

/// The gzip magic number (`1F 8B`), used to sniff world-saved template files.
const fn gzip_magic() -> &'static [u8; 2] {
    &[0x1F, 0x8B]
}

/// Where a saved structure template lives below the world's `generated`
/// directory, mirroring vanilla's `TemplatePathFactory` and the 26.3
/// `WORLD_STRUCTURE_LISTER` (`structure` prefix, `.nbt` suffix).
fn template_file(
    generated_dir: &Path,
    identifier: &Identifier,
) -> Result<std::path::PathBuf, TemplateSaveError> {
    let file = generated_dir
        .join(identifier.namespace())
        .join("structure")
        .join(format!("{}.nbt", identifier.path()));
    for segment in identifier.path().split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.starts_with('.')
            || segment.contains('\\')
            || segment.contains('\u{0}')
        {
            return Err(TemplateSaveError::InvalidName(identifier.to_string()));
        }
    }
    Ok(file)
}

/// Vanilla `StructureTemplateManager#save`: zlib-compressed NBT with an empty
/// root name, matching vanilla's `NbtIo.writeCompressed` output.
fn write_template_compressed(
    file: &Path,
    template: &StructureTemplate,
) -> Result<(), std::io::Error> {
    let nbt = template.save();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    Nbt::new(String::new(), nbt).write_to_writer(&mut encoder)?;
    let bytes = encoder.finish()?;
    std::fs::write(file, bytes)?;
    Ok(())
}

/// Errors from [`TemplateCache::save`].
#[derive(Debug)]
pub enum TemplateSaveError {
    /// The template name could not be turned into a file path.
    InvalidName(String),
    /// Writing the template file failed.
    Io(std::io::Error),
}

impl std::fmt::Display for TemplateSaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidName(name) => write!(f, "can not save structure template '{name}'"),
            Self::Io(error) => write!(f, "saving structure template failed: {error}"),
        }
    }
}

impl std::error::Error for TemplateSaveError {}

impl From<std::io::Error> for TemplateSaveError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// Global template cache instance.
///
/// This provides a singleton cache that can be used throughout the codebase
/// without needing to pass around a cache reference.
static GLOBAL_CACHE: std::sync::LazyLock<TemplateCache> =
    std::sync::LazyLock::new(TemplateCache::new);

/// Gets the global template cache.
#[must_use]
pub fn global_cache() -> &'static TemplateCache {
    &GLOBAL_CACHE
}

/// Gets a template by `name` from the global cache.
///
/// Returns the loaded template wrapped in an `Arc`, or `None` if not found.
#[must_use]
pub fn get_template(name: &str) -> Option<Arc<StructureTemplate>> {
    global_cache().get(name)
}

/// Returns a list of all available template names that can be loaded.
///
/// These are derived from the embedded structure files at compile time.
/// Names are fully qualified (e.g. `minecraft:village/plains/houses/...`).
/// Useful for tab-completion in commands.
#[must_use]
pub const fn all_template_names() -> &'static [&'static str] {
    pumpkin_data::structure_template::all_template_names()
}

/// Returns a list of all available structure names for `/place structure` tab-completion.
#[must_use]
pub const fn all_structure_names() -> &'static [&'static str] {
    pumpkin_data::structures::StructureKeys::all_names()
}

/// Returns a list of all available pool names for `/place jigsaw` tab-completion.
#[must_use]
pub const fn all_pool_names() -> &'static [&'static str] {
    pumpkin_data::template_pool::StaticTemplatePool::all_names()
}

#[must_use]
pub const fn all_embedded_datapack_names() -> &'static [&'static str] {
    pumpkin_data::structure_template::all_embedded_datapack_names()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::math::vector3::Vector3;

    #[test]
    fn save_load_round_trip_keeps_the_template() {
        let cache = TemplateCache::new();
        let mut template = StructureTemplate::default();
        template.size = Vector3::new(3, 2, 4);

        let generated_dir = std::env::temp_dir().join("pumpkin-template-cache-test");
        let _ = std::fs::remove_dir_all(&generated_dir);
        cache
            .save("minecraft:test_saving", &template, &generated_dir)
            .expect("template must save");

        // The file lands at vanilla's `generated/<namespace>/structure` path
        // with the same encoding the vanilla server writes.
        let file = generated_dir
            .join("minecraft")
            .join("structure")
            .join("test_saving.nbt");
        assert!(file.exists());
        assert!(std::fs::read(&file).is_ok_and(|bytes| !bytes.starts_with(&[0x1F, 0x8B])));

        // And it reloads through the world-generated scan path.
        let loaded = cache
            .load_from_disk(&file)
            .expect("saved template must reload");
        // Vanilla keeps the author out of the template file since the 26.2
        // format, so only the size round-trips.
        assert_eq!(loaded.size, Vector3::new(3, 2, 4));

        // Non-portable names are rejected before any file is written.
        assert!(
            cache
                .save("minecraft:..", &template, &generated_dir)
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&generated_dir);
    }

    #[test]
    fn store_and_remove_manage_the_cache_entry() {
        let cache = TemplateCache::new();
        let mut template = StructureTemplate::default();
        template.set_author("tester".to_owned());
        cache.store("test_storing", template);
        assert!(cache.get("minecraft:test_storing").is_some());
        cache.remove("test_storing");
        assert!(cache.get("test_storing").is_none());
    }
}
