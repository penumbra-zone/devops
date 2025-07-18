use directories::ProjectDirs;
use std::path::PathBuf;

pub fn default_home() -> PathBuf {
    let path: PathBuf = ProjectDirs::from("zone", "penumbra", "pindexer-db-wrangler")
        .expect("failed to get platform data dir")
        .data_dir()
        .to_path_buf();
    path
    // Utf8PathBuf::from_path_buf(path).expect(msg: "Platform default data dir was not UTF-8")
}
