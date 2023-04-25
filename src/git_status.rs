use git2::DiffOptions;

use crate::config::Config;

#[derive(Debug)]
pub struct PathStatus {
    pub delta: git2::Delta,
    pub path: String,
    pub original_path: Option<String>,
}

pub fn git_status(config: &Config) -> Result<Vec<PathStatus>, String> {
    let repo = &config.git_repo;
    let index = repo.index().unwrap();
    let head_tree = repo.head().unwrap().peel_to_tree().unwrap();

    let mut diff_options = DiffOptions::new();
    diff_options.include_typechange(true);
    diff_options.include_typechange_trees(true);

    let diff = repo.diff_tree_to_index(
        Some(&head_tree),
        Some(&index),
        Some(&mut diff_options),
    ).unwrap();

    let mut changes: Vec<PathStatus> = vec![];

    for delta in diff.deltas() {
        let path = match delta.new_file().path() {
            Some(path) => {
                match path.to_str() {
                    Some(path_str) => path_str.to_owned(),
                    None => Err(format!("Path could not be converted to a string: {:?}", path))?,
                }
            },
            None => Err(format!("Delta is missing path: {:?}", delta))?,
        };

        let original_path = match delta.old_file().path() {
            Some(path) => {
                match path.to_str() {
                    Some(path_str) => Some(path_str.to_owned()),
                    None => Err(format!("Path could not be converted to a string: {:?}", path))?,
                }
            }
            None => None,
        };

        let path_status = PathStatus {
            delta: delta.status(),
            path: path,
            original_path: original_path,
        };

        changes.push(path_status);
    }

    Ok(changes)
}
