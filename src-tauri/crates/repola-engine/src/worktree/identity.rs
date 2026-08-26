use std::path::Path;

use uuid::Uuid;

const REPOLA_NAMESPACE: Uuid = Uuid::from_u128(0x6f5e8ef2_9771_4ccb_8447_5197f199bd55);

pub(super) fn repository_id(git_common_dir: &Path) -> String {
    scoped_path_id(b"repository\0", git_common_dir)
}

pub(super) fn worktree_id(repository_id: &str, worktree_path: &Path) -> String {
    let mut identity =
        Vec::with_capacity(repository_id.len() + 10 + path_bytes(worktree_path).len());
    identity.extend_from_slice(b"worktree\0");
    identity.extend_from_slice(repository_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(&path_bytes(worktree_path));
    Uuid::new_v5(&REPOLA_NAMESPACE, &identity).to_string()
}

fn scoped_path_id(scope: &[u8], path: &Path) -> String {
    let bytes = path_bytes(path);
    let mut identity = Vec::with_capacity(scope.len() + bytes.len());
    identity.extend_from_slice(scope);
    identity.extend_from_slice(&bytes);
    Uuid::new_v5(&REPOLA_NAMESPACE, &identity).to_string()
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_stable_scoped_and_collision_resistant() {
        let repository = repository_id(Path::new("/code/example/.git"));
        assert_eq!(repository, repository_id(Path::new("/code/example/.git")));
        assert_ne!(repository, repository_id(Path::new("/code/other/.git")));
        let primary = worktree_id(&repository, Path::new("/code/example"));
        assert_eq!(
            primary,
            worktree_id(&repository, Path::new("/code/example"))
        );
        assert_ne!(
            primary,
            worktree_id(&repository, Path::new("/code/example-copy"))
        );
        assert!(Uuid::parse_str(&repository).is_ok());
        assert!(Uuid::parse_str(&primary).is_ok());
    }
}
