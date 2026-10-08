use std::fs;
use std::path::Path;
use std::sync::Arc;
use tracing::error;

/// Recursively scans `<namespace_dir>/structure/` for `.nbt` structure templates
/// and registers them into [`pumpkin_world::generation::structure::template::global_cache()`].
#[must_use]
pub fn load_structures_from_dir(namespace: &str, structure_dir: &Path) -> usize {
    let mut count = 0;
    visit_structure_dir(structure_dir, namespace, "", &mut count);
    count
}

/// Vanilla keeps a `DirectoryTemplateSource` over the world's `generated`
/// folder, so templates saved by structure blocks remain loadable across
/// restarts.
///
/// Scans `<world_root>/generated/<namespace>/structure{,s}/` and stores each
/// found template, fully parsed, into the global cache. Vanilla's zlib
/// encoding and Pumpkin's gzip encoding are both accepted.
#[must_use]
pub fn load_world_generated_structures(world_root: &Path) -> usize {
    let generated_dir = world_root.join("generated");
    let Ok(namespaces) = fs::read_dir(&generated_dir) else {
        return 0;
    };

    let mut count = 0;
    for namespace in namespaces.flatten() {
        let namespace = namespace.file_name().to_string_lossy().into_owned();
        for structure_sub in ["structure", "structures"] {
            let structure_dir = namespace_path_of(&generated_dir, &namespace, structure_sub);
            if structure_dir.is_dir() {
                count += visit_generated_structure_dir(&namespace, &structure_dir, "");
            }
        }
    }
    count
}

fn namespace_path_of(generated_dir: &Path, namespace: &str, sub: &str) -> std::path::PathBuf {
    generated_dir.join(namespace).join(sub)
}

fn visit_generated_structure_dir(namespace: &str, dir: &Path, prefix: &str) -> usize {
    let mut count = 0;
    let Ok(entries) = fs::read_dir(dir) else {
        return count;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let next_prefix = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            count += visit_generated_structure_dir(namespace, &path, &next_prefix);
            continue;
        }

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("nbt"))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        let stem = stem.to_string_lossy();
        let template_name = if prefix.is_empty() {
            stem.into_owned()
        } else {
            format!("{prefix}/{stem}")
        };
        let resource_id = format!("{namespace}:{template_name}");

        let cache = pumpkin_world::generation::structure::template::global_cache();
        if let Some(template) = cache.load_from_disk(&path) {
            cache.store(&resource_id, template);
            count += 1;
        } else {
            error!(
                "Failed to load world-generated structure template '{resource_id}' from {:?}",
                path
            );
        }
    }
    count
}

fn visit_structure_dir(dir: &Path, namespace: &str, prefix: &str, count: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        if path.is_dir() {
            let next_prefix = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            visit_structure_dir(&path, namespace, &next_prefix, count);
            continue;
        }

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("nbt"))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        let stem = stem.to_string_lossy();
        let template_name = if prefix.is_empty() {
            stem.to_string()
        } else {
            format!("{prefix}/{stem}")
        };

        let resource_id = format!("{namespace}:{template_name}");
        match fs::read(&path) {
            Ok(bytes) => {
                pumpkin_world::generation::structure::template::global_cache()
                    .register_template(&resource_id, Arc::from(bytes.into_boxed_slice()));
                *count += 1;
            }
            Err(e) => {
                error!("Failed to read structure template '{resource_id}' from {path:?}: {e}");
            }
        }
    }
}

/// Recursively scans `<namespace_dir>/worldgen/template_pool/` for `.json` template pool definitions.
///
/// Registers them into [`pumpkin_world::generation::structure::structures::jigsaw::register_dynamic_template_pool_json`].
#[must_use]
pub fn load_template_pools_from_dir(namespace: &str, template_pool_dir: &Path) -> usize {
    let mut count = 0;
    visit_template_pool_dir(template_pool_dir, namespace, "", &mut count);
    count
}

fn visit_template_pool_dir(dir: &Path, namespace: &str, prefix: &str, count: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        if path.is_dir() {
            let next_prefix = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            visit_template_pool_dir(&path, namespace, &next_prefix, count);
            continue;
        }

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        let stem = stem.to_string_lossy();
        let pool_name = if prefix.is_empty() {
            stem.to_string()
        } else {
            format!("{prefix}/{stem}")
        };

        let resource_id = format!("{namespace}:{pool_name}");
        match fs::read_to_string(&path) {
            Ok(json_content) => {
                match pumpkin_world::generation::structure::structures::jigsaw::register_dynamic_template_pool_json(
                    &resource_id,
                    &json_content,
                ) {
                    Ok(_) => {
                        *count += 1;
                    }
                    Err(e) => {
                        error!("Failed to parse template pool '{resource_id}' from {path:?}: {e}");
                    }
                }
            }
            Err(e) => {
                error!("Failed to read template pool JSON '{resource_id}' from {path:?}: {e}");
            }
        }
    }
}

/// Recursively scans `<namespace_dir>/worldgen/processor_list/` for `.json` processor lists.
///
/// Registers them into [`pumpkin_world::generation::structure::template::processor::register_dynamic_processor_list`].
#[must_use]
pub fn load_processor_lists_from_dir(namespace: &str, processor_list_dir: &Path) -> usize {
    let mut count = 0;
    visit_processor_list_dir(processor_list_dir, namespace, "", &mut count);
    count
}

fn visit_processor_list_dir(dir: &Path, namespace: &str, prefix: &str, count: &mut usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };

        if path.is_dir() {
            let next_prefix = if prefix.is_empty() {
                file_name
            } else {
                format!("{prefix}/{file_name}")
            };
            visit_processor_list_dir(&path, namespace, &next_prefix, count);
            continue;
        }

        if !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            continue;
        }

        let Some(stem) = path.file_stem() else {
            continue;
        };
        let stem = stem.to_string_lossy();
        let list_name = if prefix.is_empty() {
            stem.to_string()
        } else {
            format!("{prefix}/{stem}")
        };

        let resource_id = format!("{namespace}:{list_name}");
        match fs::read_to_string(&path) {
            Ok(json_content) => {
                match pumpkin_world::generation::structure::template::processor::register_dynamic_processor_list(
                    &resource_id,
                    &json_content,
                ) {
                    Ok(_) => {
                        *count += 1;
                    }
                    Err(e) => {
                        error!("Failed to parse processor list '{resource_id}' from {path:?}: {e}");
                    }
                }
            }
            Err(e) => {
                error!("Failed to read processor list JSON '{resource_id}' from {path:?}: {e}");
            }
        }
    }
}

/// Clears all dynamically registered worldgen structures, template pools, and processor lists.
pub fn clear_dynamic_worldgen_data() {
    pumpkin_world::generation::structure::template::global_cache().clear_dynamic();
    pumpkin_world::generation::structure::structures::jigsaw::clear_dynamic_template_pools();
    pumpkin_world::generation::structure::template::processor::clear_dynamic_processors();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_structure_template_and_template_pool() {
        let dir = tempdir().unwrap();
        let data_dir = dir.path().join("data").join("test_ns");
        let structure_dir = data_dir.join("structure");
        let template_pool_dir = data_dir.join("worldgen").join("template_pool");
        fs::create_dir_all(&structure_dir).unwrap();
        fs::create_dir_all(&template_pool_dir).unwrap();

        // Write a test template pool JSON
        let pool_json = r#"{
            "fallback": "minecraft:empty",
            "elements": [
                {
                    "weight": 10,
                    "element": {
                        "element_type": "minecraft:empty_pool_element"
                    }
                }
            ]
        }"#;
        fs::write(template_pool_dir.join("test_pool.json"), pool_json).unwrap();

        let pools_loaded = load_template_pools_from_dir("test_ns", &template_pool_dir);
        assert_eq!(pools_loaded, 1);

        let discovered =
            pumpkin_world::generation::structure::structures::jigsaw::TemplatePool::discover(
                "test_ns:test_pool",
            );
        assert!(discovered.is_some());
        let pool = discovered.unwrap();
        assert_eq!(pool.elements.len(), 1);
        assert_eq!(pool.elements[0].weight, 10);

        clear_dynamic_worldgen_data();
    }
}
