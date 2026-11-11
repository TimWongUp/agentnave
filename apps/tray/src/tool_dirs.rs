use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone)]
pub(crate) struct ToolDirs {
    pub home: PathBuf,
    pub tools: PathBuf,
    pub bin: PathBuf,
    pub network: BTreeMap<String, String>,
}
