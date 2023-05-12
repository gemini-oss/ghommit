use git2::{Delta, DiffOptions, FileMode, Repository};

#[derive(Debug, Eq, PartialEq)]
pub struct PathStatus {
    pub delta: Delta,
    pub file_mode: FileMode,
    pub path: String,
    pub original_path: Option<String>,
}

pub fn git_status(repo: &Repository) -> Result<Vec<PathStatus>, String> {
    let index = repo.index()
        .map_err(|e| format!("Unable to read git index: {}", e.to_string()))?;
    let head = repo.head()
        .map_err(|e| format!("Unable to read git head: {}", e.to_string()))?;
    let head_tree = head.peel_to_tree()
        .map_err(|e| format!("Unable to peel git head to tree: {}", e.to_string()))?;

    let mut diff_options = DiffOptions::new();
    diff_options.include_typechange(true);
    diff_options.include_typechange_trees(true);

    let diff = repo.diff_tree_to_index(
        Some(&head_tree),
        Some(&index),
        Some(&mut diff_options),
    ).map_err(|e| format!("Unable to create diff between head tree and index: {}", e.to_string()))?;

    let mut changes: Vec<PathStatus> = vec![];

    for diff_delta in diff.deltas() {
        let delta = diff_delta.status();
        let file_mode = diff_delta.new_file().mode();

        let path = match diff_delta.new_file().path() {
            Some(path) => {
                match path.to_str() {
                    Some(path_str) => path_str.to_owned(),
                    None => Err(format!("Path could not be converted to a string: {:?}", path))?,
                }
            },
            None => Err(format!("Delta is missing path: {:?}", diff_delta))?,
        };

        let original_path = match diff_delta.old_file().path() {
            Some(path) => {
                match path.to_str() {
                    Some(path_str) => Some(path_str.to_owned()),
                    None => Err(format!("Path could not be converted to a string: {:?}", path))?,
                }
            }
            None => None,
        };

        let path_status = PathStatus {
            delta: delta,
            file_mode: file_mode,
            path: path,
            original_path: original_path,
        };

        changes.push(path_status);
    }

    Ok(changes)
}
