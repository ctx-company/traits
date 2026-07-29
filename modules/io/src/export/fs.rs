use super::{control, host, policy};
use camino::Utf8Path;
pub struct Service;
impl control::Interface for Service {
    fn write(&self, request: control::Request<'_>) -> control::Result<control::Response> {
        policy::write(self, request)
    }
}
impl host::Interface for Service {
    fn probe(&self, path: &Utf8Path) -> std::io::Result<host::PathState> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Ok(host::PathState::Symlink),
            Ok(metadata) if metadata.file_type().is_dir() => Ok(host::PathState::Directory),
            Ok(metadata) if metadata.file_type().is_file() => Ok(host::PathState::File),
            Ok(_) => Ok(host::PathState::Other),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(host::PathState::Missing)
            }
            Err(error) => Err(error),
        }
    }
    fn read_text(&self, path: &Utf8Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }
    fn read_bytes(&self, path: &Utf8Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
    fn create_dir_all(&self, path: &Utf8Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }
    fn write(&self, path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()> {
        crate::write::write_bytes_atomically_raw(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::{
        Format, Identity, OwnershipKey,
        control::{Interface as _, Request},
    };
    use camino::Utf8PathBuf;
    use ctx_traits_core::{digest::Digest, r#trait::Id};

    fn temp_root(name: &str) -> Utf8PathBuf {
        let unique = format!(
            "ctx-traits-export-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos()
        );
        Utf8PathBuf::from_path_buf(std::env::temp_dir().join(unique)).expect("UTF-8 temporary path")
    }

    fn identity() -> Identity {
        Identity::new(
            Id::new("example-trait").expect("valid ID"),
            Digest::from_bytes(b"source"),
            OwnershipKey::Skill,
        )
    }

    #[test]
    fn creates_parent_writes_exact_bytes_and_returns_byte_evidence() {
        let root = temp_root("bytes");
        let identity = identity();
        let content = "hello, \u{1f642}";
        let response = Service
            .write(Request::new(&root, &identity, content, Format::Skill))
            .expect("write succeeds");
        assert_eq!(response.path, root.join("example-trait/SKILL.md"));
        assert_eq!(response.ownership, OwnershipKey::Skill);
        assert_eq!(
            response.content_digest,
            Digest::from_bytes(content.as_bytes())
        );
        assert_eq!(response.byte_size, content.len() as u64);
        assert_eq!(
            std::fs::read(&response.path).expect("written file"),
            content.as_bytes()
        );
        std::fs::remove_dir_all(root).expect("clean temporary directory");
    }

    #[test]
    fn non_utf8_existing_file_is_an_io_error() {
        let root = temp_root("non-utf8");
        let identity = identity();
        let target = root.join("example-trait/SKILL.md");
        std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
        std::fs::write(&target, [0xff]).expect("write invalid text");
        let error = match Service.write(Request::new(&root, &identity, "new", Format::Skill)) {
            Ok(_) => panic!("invalid UTF-8 is not managed text"),
            Err(error) => error,
        };
        assert!(
            matches!(error, crate::export::Error::Io { source, .. } if source.kind() == std::io::ErrorKind::InvalidData)
        );
        std::fs::remove_dir_all(root).expect("clean temporary directory");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_leaf_and_ancestor_are_refused() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink");
        let identity = identity();
        std::fs::create_dir_all(&root).expect("create root");
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(&outside).expect("create outside");
        symlink(&outside, root.join("example-trait")).expect("create ancestor symlink");
        assert!(matches!(
            Service.write(Request::new(&root, &identity, "new", Format::Skill)),
            Err(crate::export::Error::SymlinkAncestor { .. })
        ));
        std::fs::remove_file(root.join("example-trait")).expect("remove ancestor symlink");
        std::fs::create_dir_all(root.join("example-trait")).expect("create parent");
        symlink(outside.join("missing"), root.join("example-trait/SKILL.md"))
            .expect("create leaf symlink");
        assert!(matches!(
            Service.write(Request::new(&root, &identity, "new", Format::Skill)),
            Err(crate::export::Error::LeafSymlink { .. })
        ));
        std::fs::remove_dir_all(root).expect("clean root");
        std::fs::remove_dir_all(outside).expect("clean outside");
    }
}
