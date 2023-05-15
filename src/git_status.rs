use git2::{Delta, DiffOptions, FileMode, Index, ObjectType, Oid, Repository};

#[derive(Debug, Eq, PartialEq)]
pub struct PathStatus {
    pub delta: Delta,
    pub file_mode: FileMode,
    pub object_id: Oid,
    pub object_type: Option<ObjectType>,
    pub original_path: Option<String>,
    pub path: String,
}

/// Currently conflicts are not supported. Should that change in the future,
/// this function would likely be removed and other changes would need to be
/// made to accommodate.
///
/// https://git-scm.com/docs/revisions/2.39.3#Documentation/revisions.txt-emltngtltpathgtemegem0READMEememREADMEem
fn stage_number(index: &Index) -> Result<i32, String> {
    if index.has_conflicts() {
        Err("Handling conflicts is not supported, and conflicts were detected".to_owned())
    } else {
        Ok(0)
    }
}

pub fn git_status(repo: &Repository) -> Result<Vec<PathStatus>, String> {
    let index = repo.index()
        .map_err(|e| format!("Unable to read git index: {}", e))?;

    let stage_number = stage_number(&index)?;

    let head = repo.head()
        .map_err(|e| format!("Unable to read git head: {}", e))?;
    let head_tree = head.peel_to_tree()
        .map_err(|e| format!("Unable to peel git head to tree: {}", e))?;

    let mut diff_options = DiffOptions::new();
    diff_options.include_typechange(true);
    diff_options.include_typechange_trees(true);

    let diff = repo.diff_tree_to_index(
        Some(&head_tree),
        Some(&index),
        Some(&mut diff_options),
    ).map_err(|e| format!("Unable to create diff between head tree and index: {}", e))?;

    let mut changes: Vec<PathStatus> = vec![];

    for diff_delta in diff.deltas() {
        let new_file = diff_delta.new_file();

        let delta = diff_delta.status();
        let file_mode = new_file.mode();
        let object_id = new_file.id();

        let new_path = new_file.path()
            .ok_or_else(|| format!("Delta is missing path: {:?}", diff_delta))?;

        let path_string = match new_path.to_str() {
            Some(path_str) => path_str.to_owned(),
            None => Err(format!("Path could not be converted to a string: {:?}", new_path))?,
        };

        let object_type = match index.get_path(new_path, stage_number) {
            Some(index_entry) => {
                match repo.find_object(index_entry.id, None) {
                    Ok(object) => object.kind(),
                    Err(_) => Err(format!("Unable to find object with ID {}", index_entry.id))?,
                }
            },
            // - Deleted files will not have a tree entry
            None => None,
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
            object_id: object_id,
            object_type: object_type,
            original_path: original_path,
            path: path_string,
        };

        changes.push(path_status);
    }

    Ok(changes)
}

#[cfg(test)]
mod git_status_tests {
    use git2::FileMode;

    use crate::test_utils::test_utils::{DELETED_FILE_OID, TempGitRepo, git_hash_object_stdin, path_to_str};
    use super::{PathStatus, git_status};

    /// This is O(n^2). Since inputs are small, this shouldn't be an issue, but
    /// if it becomes an issue, consider implementing the traits necessary so
    /// that the `Vec`s can either be:
    /// - Sorted
    /// - Converted to sets
    fn assert_eq_order_independent(a: &Vec<PathStatus>, b: &Vec<PathStatus>) {
        assert_eq!(a.len(), b.len());

        assert_eq!(a.iter().all(|item| b.contains(item)), true);
        assert_eq!(b.iter().all(|item| a.contains(item)), true);
    }

    #[test]
    fn added_file() {
        let repo = TempGitRepo::new();

        let foo_contents = "foo\n";
        let foo = repo.create_or_replace_blob_file("foo", foo_contents.as_bytes());

        repo.git_add(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = path_to_str(&foo);
            let object_id = git_hash_object_stdin(foo_contents);

            vec![
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Blob,
                    object_id: object_id,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    #[cfg(unix)]
    fn added_symlink() {
        let repo = TempGitRepo::new();

        let foo_contents = "foo\n";
        let foo_path = "foo";
        let foo = repo.create_or_replace_blob_file(foo_path, foo_contents.as_bytes());
        let bar = repo.create_or_replace_symlink_file("bar", foo_path);

        repo.git_add(&foo);
        repo.git_add(&bar);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let foo_path = path_to_str(&foo);
            let foo_object_id = git_hash_object_stdin(foo_contents);
            let bar_path = path_to_str(&bar);
            let bar_object_id = git_hash_object_stdin(foo_path);

            vec![
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Blob,
                    object_id: foo_object_id,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(foo_path.to_string()),
                    path: foo_path.to_string(),
                },
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Link,
                    object_id: bar_object_id,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(bar_path.to_string()),
                    path: bar_path.to_string(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    fn modified_file() {
        let repo = TempGitRepo::new();

        let foo = repo.create_or_replace_blob_file("foo", "foo\n".as_bytes());

        repo.git_add(&foo);
        repo.git_commit("Adding foo");

        let foo_contents = "foo\nfoo\n";
        let foo = repo.create_or_replace_blob_file("foo", foo_contents.as_bytes());

        repo.git_add(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = path_to_str(&foo);
            let object_id = git_hash_object_stdin(foo_contents);

            vec![
                PathStatus {
                    delta: git2::Delta::Modified,
                    file_mode: FileMode::Blob,
                    object_id: object_id,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    fn deleted_file() {
        let repo = TempGitRepo::new();

        let foo = repo.create_or_replace_blob_file("foo", "foo\n".as_bytes());

        repo.git_add(&foo);
        repo.git_commit("Adding foo");

        repo.git_rm(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let path = path_to_str(&foo);
            let object_id = *DELETED_FILE_OID;

            vec![
                PathStatus {
                    delta: git2::Delta::Deleted,
                    file_mode: FileMode::Unreadable,
                    object_id: object_id,
                    object_type: None,
                    original_path: Some(path.to_owned()),
                    path: path.to_owned(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    #[cfg(unix)]
    fn typechange_test() {
        let repo = TempGitRepo::new();

        let foo_path = "foo";
        let foo = repo.create_or_replace_blob_file(foo_path, "foo\n".as_bytes());
        let bar = repo.create_or_replace_symlink_file("bar", foo_path);

        repo.git_add(&foo);
        repo.git_add(&bar);
        repo.git_commit("Add foo as blob and bar as symlink");

        let bar_contents = "bar\n";
        let bar = repo.create_or_replace_blob_file("bar", bar_contents.as_bytes());

        repo.git_add(&bar);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let bar_path = path_to_str(&bar);
            let bar_object_id = git_hash_object_stdin(bar_contents);

            vec![
                PathStatus {
                    delta: git2::Delta::Typechange,
                    file_mode: FileMode::Blob,
                    object_id: bar_object_id,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(bar_path.to_string()),
                    path: bar_path.to_string(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }

    #[test]
    #[cfg(unix)]
    fn multiple_changes() {
        let repo = TempGitRepo::new();

        let foo = repo.create_or_replace_blob_file("foo", "foo\n".as_bytes());
        let bar = repo.create_or_replace_blob_file("bar", "bar\n".as_bytes());
        let quux = repo.create_or_replace_symlink_file("quux", "foo");

        repo.git_add(&foo);
        repo.git_add(&bar);
        repo.git_add(&quux);
        repo.git_commit("Adding foo and bar");

        let bar_content = "bar\nbar\n";
        let bar = repo.create_or_replace_blob_file("bar", bar_content.as_bytes());

        let baz_content = "baz\n";
        let baz = repo.create_or_replace_blob_file("baz", baz_content.as_bytes());

        let quux_content = "quux\n";
        let quux = repo.create_or_replace_blob_file("quux", quux_content.as_bytes());

        repo.git_add(&bar);
        repo.git_add(&baz);
        repo.git_add(&quux);
        repo.git_rm(&foo);

        let actual = git_status(&repo.repo)
            .expect("Unable to get a git status");

        let expected = {
            let foo_path = path_to_str(&foo);
            let foo_oid = *DELETED_FILE_OID;

            let bar_path = path_to_str(&bar);
            let bar_oid = git_hash_object_stdin(&bar_content);

            let baz_path = path_to_str(&baz);
            let baz_oid = git_hash_object_stdin(&baz_content);

            let quux_path = path_to_str(&quux);
            let quux_oid = git_hash_object_stdin(&quux_content);

            vec![
                PathStatus {
                    delta: git2::Delta::Added,
                    file_mode: FileMode::Blob,
                    object_id: baz_oid,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(baz_path.to_owned()),
                    path: baz_path.to_owned(),
                },
                PathStatus {
                    delta: git2::Delta::Modified,
                    file_mode: FileMode::Blob,
                    object_id: bar_oid,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(bar_path.to_owned()),
                    path: bar_path.to_owned(),
                },
                PathStatus {
                    delta: git2::Delta::Deleted,
                    file_mode: FileMode::Unreadable,
                    object_id: foo_oid,
                    object_type: None,
                    original_path: Some(foo_path.to_owned()),
                    path: foo_path.to_owned(),
                },
                PathStatus {
                    delta: git2::Delta::Typechange,
                    file_mode: FileMode::Blob,
                    object_id: quux_oid,
                    object_type: Some(git2::ObjectType::Blob),
                    original_path: Some(quux_path.to_string()),
                    path: quux_path.to_string(),
                },
            ]
        };

        assert_eq_order_independent(&actual, &expected);
    }
}
