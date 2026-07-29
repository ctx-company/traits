use camino::Utf8Path;
#[derive(Clone, Copy)]
pub(super) enum PathState {
    Missing,
    Directory,
    File,
    Symlink,
    Other,
}
pub(super) trait Interface {
    fn probe(&self, path: &Utf8Path) -> std::io::Result<PathState>;
    fn read_text(&self, path: &Utf8Path) -> std::io::Result<String>;
    fn read_bytes(&self, path: &Utf8Path) -> std::io::Result<Vec<u8>>;
    fn create_dir_all(&self, path: &Utf8Path) -> std::io::Result<()>;
    fn write(&self, path: &Utf8Path, bytes: &[u8]) -> std::io::Result<()>;
}

#[cfg(test)]
pub(super) mod memory {
    use super::{Interface, PathState};
    use camino::{Utf8Path, Utf8PathBuf};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::io;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(in crate::export) enum Operation {
        Probe(Utf8PathBuf),
        ReadText(Utf8PathBuf),
        ReadBytes(Utf8PathBuf),
        CreateDirAll(Utf8PathBuf),
        Write(Utf8PathBuf, Vec<u8>),
    }

    pub(in crate::export) struct Service {
        states: RefCell<BTreeMap<Utf8PathBuf, PathState>>,
        files: RefCell<BTreeMap<Utf8PathBuf, Vec<u8>>>,
        operations: RefCell<Vec<Operation>>,
        failure: RefCell<Option<(OperationKind, io::ErrorKind)>>,
    }

    #[derive(Clone, Copy)]
    pub(in crate::export) enum OperationKind {
        Probe,
        ReadText,
        ReadBytes,
        CreateDirAll,
        Write,
    }

    impl Service {
        pub(in crate::export) fn new() -> Self {
            Self {
                states: RefCell::new(BTreeMap::new()),
                files: RefCell::new(BTreeMap::new()),
                operations: RefCell::new(Vec::new()),
                failure: RefCell::new(None),
            }
        }
        pub(in crate::export) fn state(&self, path: impl Into<Utf8PathBuf>, state: PathState) {
            self.states.borrow_mut().insert(path.into(), state);
        }
        pub(in crate::export) fn file(
            &self,
            path: impl Into<Utf8PathBuf>,
            content: impl Into<Vec<u8>>,
        ) {
            let path = path.into();
            self.state(path.clone(), PathState::File);
            self.files.borrow_mut().insert(path, content.into());
        }
        pub(in crate::export) fn fail(&self, operation: OperationKind, kind: io::ErrorKind) {
            *self.failure.borrow_mut() = Some((operation, kind));
        }
        pub(in crate::export) fn operations(&self) -> Vec<Operation> {
            self.operations.borrow().clone()
        }
        pub(in crate::export) fn written(&self, path: &Utf8Path) -> Option<Vec<u8>> {
            self.files.borrow().get(path).cloned()
        }
        fn maybe_fail(&self, kind: OperationKind) -> io::Result<()> {
            if self
                .failure
                .borrow()
                .is_some_and(|(operation, _)| operation as u8 == kind as u8)
            {
                let (_, error_kind) = self.failure.borrow_mut().take().expect("failure exists");
                return Err(io::Error::from(error_kind));
            }
            Ok(())
        }
    }

    impl Interface for Service {
        fn probe(&self, path: &Utf8Path) -> io::Result<PathState> {
            self.operations
                .borrow_mut()
                .push(Operation::Probe(path.to_owned()));
            self.maybe_fail(OperationKind::Probe)?;
            Ok(self
                .states
                .borrow()
                .get(path)
                .copied()
                .unwrap_or(PathState::Missing))
        }
        fn read_text(&self, path: &Utf8Path) -> io::Result<String> {
            self.operations
                .borrow_mut()
                .push(Operation::ReadText(path.to_owned()));
            self.maybe_fail(OperationKind::ReadText)?;
            String::from_utf8(self.files.borrow().get(path).cloned().unwrap_or_default())
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        }
        fn read_bytes(&self, path: &Utf8Path) -> io::Result<Vec<u8>> {
            self.operations
                .borrow_mut()
                .push(Operation::ReadBytes(path.to_owned()));
            self.maybe_fail(OperationKind::ReadBytes)?;
            Ok(self.files.borrow().get(path).cloned().unwrap_or_default())
        }
        fn create_dir_all(&self, path: &Utf8Path) -> io::Result<()> {
            self.operations
                .borrow_mut()
                .push(Operation::CreateDirAll(path.to_owned()));
            self.maybe_fail(OperationKind::CreateDirAll)?;
            self.states
                .borrow_mut()
                .insert(path.to_owned(), PathState::Directory);
            Ok(())
        }
        fn write(&self, path: &Utf8Path, bytes: &[u8]) -> io::Result<()> {
            self.operations
                .borrow_mut()
                .push(Operation::Write(path.to_owned(), bytes.to_vec()));
            self.maybe_fail(OperationKind::Write)?;
            self.state(path.to_owned(), PathState::File);
            self.files
                .borrow_mut()
                .insert(path.to_owned(), bytes.to_vec());
            Ok(())
        }
    }
}
