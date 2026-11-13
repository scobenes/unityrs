#![allow(dead_code)]
use std::{io::Read, path::PathBuf, sync::Arc};
use unityrs_batch::{
    archive::{self, Context, Input, Payloads, Summary},
    digest::Hashes,
    phase::{Account, Error},
    spool::{
        Counter, FileSystem, Host,
        workspace::{Sealed, Workspace},
    },
    winners::layout::Layout,
};
use unityrs_testkit::scratch::Scratch;

pub struct Files {
    pub paths: Vec<PathBuf>,
}
impl Payloads for Files {
    fn open(&self, source: u32, ordinal: u64) -> Result<Box<dyn Read + Send>, Error> {
        if source != 0 {
            return Err(Error::internal());
        }
        let path = self
            .paths
            .get(usize::try_from(ordinal).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        Host.open(path)
            .map_err(|e| Error::from(std::io::Error::from(e.kind())))
    }
}
pub struct Scene {
    pub scratch: Scratch,
    pub workspace: Workspace,
    pub counter: Arc<Counter>,
    pub files: Files,
    pub list: Sealed,
    pub size: u64,
    pub count: u64,
}
/// Independent included-list encoder. Keep Rust header/layout encoding out of
/// the Python oracle; this helper encodes only B6's private record framing.
pub fn frame(name: &str, ordinal: u64, bytes: &[u8]) -> Vec<u8> {
    let hashes = Hashes::of(bytes);
    let mut payload = vec![0, 0];
    payload.extend(0_u32.to_le_bytes()); // archive index
    payload.extend(0_u32.to_le_bytes()); // source
    payload.extend(ordinal.to_le_bytes());
    payload.extend(u64::try_from(bytes.len()).unwrap().to_le_bytes());
    payload.extend(hashes.crc32.to_le_bytes());
    payload.extend(hashes.md5);
    for text in ["archive", name, name, "input"] {
        payload.extend(u16::try_from(text.len()).unwrap().to_le_bytes());
        payload.extend(text.as_bytes());
    }
    let mut out = u32::try_from(payload.len() + 4)
        .unwrap()
        .to_le_bytes()
        .to_vec();
    out.extend(payload);
    out
}
pub fn seal(workspace: &mut Workspace, data: &[u8]) -> Sealed {
    let mut file = workspace.create_file(&|| false).unwrap();
    file.append(data, &|| false).unwrap();
    file.seal(&|| false).unwrap()
}
pub fn list_header() -> Vec<u8> {
    let mut out = b"URSSORT\0".to_vec();
    out.extend(1_u32.to_le_bytes());
    out.extend(114_u32.to_le_bytes());
    out
}
impl Scene {
    pub fn new(members: &[(&str, &[u8])]) -> Self {
        Self::with_fs(members, 4096, Arc::new(Host))
    }
    pub fn with_fs(members: &[(&str, &[u8])], block: u64, fs: Arc<dyn FileSystem>) -> Self {
        let scratch = Scratch::new("archive-stream");
        let counter = Counter::new(u64::MAX, 0, block).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter.clone(), fs).unwrap();
        let mut data = list_header();
        let mut paths = Vec::new();
        let mut layout = Layout::default();
        for (i, &(name, bytes)) in members.iter().enumerate() {
            let path = scratch.join(format!("payload-{i}"));
            std::fs::write(&path, bytes).unwrap();
            paths.push(path);
            data.extend(frame(name, u64::try_from(i).unwrap(), bytes));
            layout
                .member(
                    u64::try_from(name.len()).unwrap(),
                    u64::try_from(bytes.len()).unwrap(),
                )
                .unwrap();
        }
        let list = seal(&mut workspace, &data);
        Self {
            scratch,
            workspace,
            counter,
            files: Files { paths },
            list,
            size: layout.finish().unwrap().size,
            count: u64::try_from(members.len()).unwrap(),
        }
    }
    pub fn input(&self) -> Input<'_> {
        Input {
            key: "archive",
            index: 0,
            members: self.count,
            size: self.size,
            list: &self.list,
        }
    }
    pub fn context<'a>(&'a self, account: &'a Account) -> Context<'a> {
        Context {
            workspace: &self.workspace,
            payloads: &self.files,
            meter: account.meter(),
            hooks: &archive::NoHooks,
        }
    }
    pub fn generate(&self, chunk: usize) -> Result<(Vec<u8>, Summary), Error> {
        let account = Account::new(1 << 20);
        let mut stream = archive::open(self.context(&account), self.input(), &|| false)?;
        let mut out = Vec::new();
        let mut buffer = vec![0; chunk];
        loop {
            let n = stream.read(&mut buffer, &|| false)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n]);
        }
        let summary = stream.finish(&|| false)?;
        assert_eq!(account.snapshot().input.used, 0);
        Ok((out, summary))
    }
}

impl Scene {
    /// Stream records to disk without retaining a per-member test dataset.
    pub fn repeated(count: u64, payload: &[u8]) -> Self {
        let scratch = Scratch::new("archive-count");
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let mut workspace =
            Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
        let path = scratch.join("payload");
        std::fs::write(&path, payload).unwrap();
        let mut writer = workspace.create_file(&|| false).unwrap();
        writer.append(&list_header(), &|| false).unwrap();
        let mut layout = Layout::default();
        for i in 0..count {
            let name = format!("m{i:06}");
            writer.append(&frame(&name, 0, payload), &|| false).unwrap();
            layout
                .member(
                    u64::try_from(name.len()).unwrap(),
                    u64::try_from(payload.len()).unwrap(),
                )
                .unwrap();
        }
        let list = writer.seal(&|| false).unwrap();
        Self {
            scratch,
            workspace,
            counter,
            files: Files { paths: vec![path] },
            list,
            size: layout.finish().unwrap().size,
            count,
        }
    }
}
