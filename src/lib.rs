pub mod filemode;
pub mod sver_config;

use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    path::{Component, Path, PathBuf},
};

use self::filemode::FileMode;
use self::sver_config::{ProfileConfig, SverConfig};
use git2::{Oid, Repository};
use log::debug;
use sha2::{Digest, Sha256};
use sver_config::ValidationResult;

pub struct SverRepository {
    repo: Repository,
    work_dir: String,
    target_path: String,
    profile: String,
}

impl SverRepository {
    pub fn new(path: &str) -> Result<Self, Box<dyn Error>> {
        let target_path = Path::new(path);
        let repo = find_repository(target_path)?;
        let target_path = relative_path(&repo, target_path)?;
        let target_path = target_path
            .iter()
            .flat_map(|os| os.to_str())
            .collect::<Vec<_>>()
            .join("/");
        let work_dir = repo
            .workdir()
            .and_then(|p| p.to_str())
            .ok_or("bare repository")?
            .to_string();
        debug!("repository_root:{}", work_dir);
        debug!("target_path:{}", target_path);
        Ok(Self {
            repo,
            work_dir,
            target_path,
            profile: "default".to_string(),
        })
    }

    pub fn init_sver_config(&self) -> Result<String, Box<dyn Error>> {
        debug!("path:{}", self.target_path);
        let mut path_buf = PathBuf::new();
        path_buf.push(&self.target_path);
        path_buf.push("sver.toml");
        let config_path = path_buf.as_path();

        if self.repo.index()?.get_path(config_path, 0).is_some() {
            return Ok("sver.toml is already exists".into());
        }
        if !SverConfig::write_initial_config(config_path)? {
            return Ok(format!(
                "sver.toml is already exists. but not commited. path:{}",
                self.target_path
            ));
        }
        Ok(format!("sver.toml is generated. path:{}", self.target_path))
    }

    pub fn validate_sver_config(&self) -> Result<Vec<ValidationResult>, Box<dyn Error>> {
        let configs = SverConfig::load_all_configs(&self.repo)?;
        configs
            .iter()
            .for_each(|config| debug!("{}", config.config_file_path()));
        let index = self.repo.index()?;
        let result: Vec<ValidationResult> = configs
            .iter()
            .flat_map(|sver_config| {
                let target_path = sver_config.target_path.clone();
                sver_config
                    .iter()
                    .map(|(profile, config)| config.validate(&target_path, profile, &index))
                    .collect::<Vec<ValidationResult>>()
            })
            .collect();
        Ok(result)
    }

    pub fn list_sources(&self) -> Result<Vec<String>, Box<dyn Error>> {
        let entries = self.list_sorted_entries()?;
        let result: Vec<String> = entries
            .iter()
            .map(|(path, _oid)| String::from_utf8(path.clone()).unwrap())
            .collect();
        Ok(result)
    }

    pub fn calc_version(&self) -> Result<Version, Box<dyn Error>> {
        let entries = self.list_sorted_entries()?;
        let version = self.calc_hash_string(&entries)?;

        let version = Version {
            repository_root: self.work_dir.clone(),
            path: self.target_path.clone(),
            version,
        };
        Ok(version)
    }

    fn calc_hash_string(
        &self,
        source: &BTreeMap<Vec<u8>, OidAndMode>,
    ) -> Result<String, Box<dyn Error>> {
        let mut hasher = Sha256::default();
        hasher.update(self.target_path.as_bytes());
        for (path, oid_and_mode) in source {
            hasher.update(path);
            match oid_and_mode.mode {
                FileMode::Blob | FileMode::BlobExecutable | FileMode::Link => {
                    // Q. Why little endian?
                    // A. no reason.
                    hasher.update(u32::from(oid_and_mode.mode).to_le_bytes());
                    let blob = self.repo.find_blob(oid_and_mode.oid)?;
                    let content = blob.content();
                    hasher.update(content);
                    debug!(
                        "path:{}, mode:{:?}, content:{}",
                        String::from_utf8(path.clone())?,
                        oid_and_mode.mode,
                        String::from_utf8(content.to_vec())?
                    )
                }
                // Commit (Submodule の場合は参照先のコミットハッシュを計算対象に加える)
                FileMode::Commit => {
                    debug!("commit_hash?:{}", oid_and_mode.oid);
                    hasher.update(oid_and_mode.oid);
                }
                _ => {
                    debug!(
                        "unsupported mode. skipped. path:{}, mode:{:?}",
                        String::from_utf8(path.clone())?,
                        oid_and_mode.mode
                    )
                }
            }
        }
        let hash = format!("{:#x}", hasher.finalize());
        Ok(hash)
    }

    fn list_sorted_entries(&self) -> Result<BTreeMap<Vec<u8>, OidAndMode>, Box<dyn Error>> {
        let mut path_set: HashMap<String, Vec<String>> = HashMap::new();
        self.collect_path_and_excludes(&self.target_path, &mut path_set)?;
        debug!("dependency_paths:{:?}", path_set);
        let mut map = BTreeMap::new();
        for entry in self.repo.index()?.iter() {
            let containable = containable(entry.path.as_slice(), &path_set);
            debug!(
                "path:{}, containable:{}, mode:{:?}",
                String::from_utf8(entry.path.clone())?,
                containable,
                FileMode::from(entry.mode),
            );
            if containable {
                debug!("add path:{:?}", String::from_utf8(entry.path.clone()));
                map.insert(
                    entry.path,
                    OidAndMode {
                        oid: entry.id,
                        mode: entry.mode.into(),
                    },
                );
            }
        }
        Ok(map)
    }

    fn collect_path_and_excludes(
        &self,
        path: &str,
        path_and_excludes: &mut HashMap<String, Vec<String>>,
    ) -> Result<(), Box<dyn Error>> {
        if path_and_excludes.contains_key(path) {
            debug!("already added. path:{}", path.to_string());
            return Ok(());
        }
        debug!("add dep path : {}", path);

        let mut p = PathBuf::new();
        p.push(path);
        p.push("sver.toml");

        let mut current_path_and_excludes: HashMap<String, Vec<String>> = HashMap::new();

        if let Some(entry) = self.repo.index()?.get_path(p.as_path(), 0) {
            debug!("sver.toml exists. path:{:?}", String::from_utf8(entry.path));
            let default_config = ProfileConfig::load_profile(
                self.repo.find_blob(entry.id)?.content(),
                &self.profile,
            )?;
            current_path_and_excludes.insert(path.to_string(), default_config.excludes.clone());
            path_and_excludes.insert(path.to_string(), default_config.excludes);
            for dependency_path in default_config.dependencies {
                self.collect_path_and_excludes(&dependency_path, path_and_excludes)?;
            }
        } else {
            current_path_and_excludes.insert(path.to_string(), vec![]);
            path_and_excludes.insert(path.to_string(), vec![]);
        }

        // include synbolic link
        for entry in self.repo.index()?.iter() {
            let containable = containable(entry.path.as_slice(), &current_path_and_excludes);
            if containable && FileMode::Link == FileMode::from(entry.mode) {
                let path = String::from_utf8(entry.path)?;
                let mut buf = PathBuf::new();
                buf.push(path);
                buf.pop();

                let blob = self.repo.find_blob(entry.id)?;
                let link_path = String::from_utf8(blob.content().to_vec())?;
                let link_path = Path::new(&link_path);
                for link_components in link_path.components() {
                    debug!("link_component:{:?}", link_components);
                    match link_components {
                        Component::ParentDir => {
                            buf.pop();
                        }
                        Component::Normal(path) => buf.push(path),
                        Component::RootDir => {}
                        Component::CurDir => {}
                        Component::Prefix(_prefix) => {}
                    }
                }

                let link_path = buf
                    .to_str()
                    .ok_or("path is invalid")?
                    .replace(OS_SEP_STR, SEPARATOR_STR);
                debug!("collect link path. path:{}", &link_path);
                self.collect_path_and_excludes(&link_path, path_and_excludes)?;
            }
        }
        Ok(())
    }
}

pub struct Version {
    pub repository_root: String,
    pub path: String,
    pub version: String,
}

fn relative_path(repo: &Repository, path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let repo_path = repo
        .workdir()
        .and_then(|p| p.canonicalize().ok())
        .ok_or("bare repository is not supported")?;
    let current_path = path.canonicalize()?;
    let result = current_path.strip_prefix(repo_path)?.to_path_buf();
    Ok(result)
}

struct OidAndMode {
    oid: Oid,
    mode: FileMode,
}

#[cfg(target_os = "windows")]
const OS_SEP_STR: &str = "\\";
#[cfg(target_os = "linux")]
const OS_SEP_STR: &str = "/";

const SEPARATOR_STR: &str = "/";
const SEPARATOR_BYTE: &[u8] = SEPARATOR_STR.as_bytes();

fn containable(test_path: &[u8], path_set: &HashMap<String, Vec<String>>) -> bool {
    path_set.iter().any(|(include, excludes)| {
        let include_file = match_samefile_or_include_dir(test_path, include.as_bytes());
        let exclude_file = excludes.iter().any(|exclude| {
            if include.is_empty() {
                match_samefile_or_include_dir(test_path, exclude.as_bytes())
            } else {
                match_samefile_or_include_dir(
                    test_path,
                    [include.as_bytes(), SEPARATOR_BYTE, exclude.as_bytes()]
                        .concat()
                        .as_slice(),
                )
            }
        });
        include_file && !exclude_file
    })
}

fn match_samefile_or_include_dir(test_path: &[u8], path: &[u8]) -> bool {
    let is_same_file = test_path == path;
    let is_contain_path =
        path.is_empty() || test_path.starts_with([path, SEPARATOR_BYTE].concat().as_slice());
    is_same_file || is_contain_path
}

fn find_repository(from_path: &Path) -> Result<Repository, Box<dyn Error>> {
    for target_path in from_path.canonicalize()?.ancestors() {
        if let Ok(repo) = Repository::open(target_path) {
            return Ok(repo);
        }
    }
    Err("repository was not found".into())
}
