//! Independent server semantics. CI invokes the ignored binary probe explicitly;
//! a missing binary fails that probe. Owner stores are always explicit opt-in.
#![cfg(feature = "test-hooks")]
mod store_support;
use hyper::body::Bytes;
use md5::{Digest, Md5};
use object_store::{
    ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload, UpdateVersion,
    list::{PaginatedListOptions, PaginatedListStore},
    multipart::MultipartStore,
    path::Path,
};
use std::{
    io,
    net::{SocketAddr, TcpListener},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64},
    },
    time::{Duration, Instant},
};
use unityrs_batch::{
    http::{
        self,
        permit::PermitPool,
        resolve::{Addresses, Resolver},
    },
    store::{
        Kind,
        attempt::{Policy, Tries, TrySlot},
        classify::{Action, Ended},
        client::{self, Settings},
        connector::Connector,
        etag,
    },
};
use unityrs_testkit::{child, guard::DEADLOCK_GUARD};
use url::Url;

struct Fixed(SocketAddr);
impl Resolver for Fixed {
    fn resolve(&self, _: &str, _: u16) -> io::Result<Addresses> {
        Ok(Addresses::first([self.0]))
    }
}
struct Probe {
    store: client::Store,
    pool: PermitPool,
    stop: AtomicBool,
    unsettled: AtomicU64,
}
impl Probe {
    fn new(
        endpoint: &Url,
        path_style: bool,
        address: Option<SocketAddr>,
        credentials: &object_store::aws::AwsCredentialProvider,
        bucket: &str,
        region: &str,
    ) -> Self {
        let service = http::Service::new(
            tokio::runtime::Handle::current(),
            Arc::new(unityrs_batch::sys::reset::SocketTable::default()),
        )
        .unwrap();
        let service = match address {
            Some(address) => service
                .with_test_hooks(Arc::new(Fixed(address)), &[])
                .unwrap(),
            None => service,
        };
        let settings = Settings {
            bucket,
            region,
            endpoint: Some(endpoint),
            path_style,
        };
        let connector = Connector::new(
            service,
            client::origin(&settings).unwrap(),
            Duration::from_secs(5),
        );
        let store =
            client::build(&settings, Some("application/zip"), credentials, &connector).unwrap();
        Self {
            store,
            pool: PermitPool::new(1).unwrap(),
            stop: AtomicBool::new(false),
            unsettled: AtomicU64::new(0),
        }
    }
    async fn call<T, F, Fut>(&self, kind: Kind, mut op: F) -> unityrs_batch::store::attempt::Done<T>
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = object_store::Result<T>>,
    {
        let slot = TrySlot::new(self.pool.acquire().unwrap(), kind);
        let tries = Tries {
            policy: Policy {
                attempts: 1,
                timeout: Duration::from_secs(15),
                backoff_initial_ms: 0,
                backoff_max_ms: 0,
                cutoff: Some(tokio::time::Instant::now() + Duration::from_secs(20)),
            },
            control: &self.stop,
            unsettled: &self.unsettled,
        };
        tries
            .run(&slot, kind, |_| {
                let future = op();
                async move { future.await.map_err(|_| Ended::Error) }
            })
            .await
            .unwrap()
    }
    async fn put(
        &self,
        key: &Path,
        data: Bytes,
        mode: PutMode,
    ) -> unityrs_batch::store::attempt::Done<object_store::PutResult> {
        self.call(
            Kind::HeadWrite {
                if_match: matches!(mode, PutMode::Update(_)),
            },
            || {
                self.store.put_opts(
                    key,
                    PutPayload::from(data.clone()),
                    PutOptions {
                        mode: mode.clone(),
                        ..Default::default()
                    },
                )
            },
        )
        .await
    }
}
async fn probes(probe: &Probe, prefix: &str) {
    conditional_probe(probe, prefix).await;
    listing_probe(probe, prefix).await;
    multipart_probe(probe, prefix).await;
}
async fn conditional_probe(probe: &Probe, prefix: &str) {
    let key = Path::parse(format!("{prefix}conditional")).unwrap();
    let done = probe
        .put(&key, Bytes::from_static(b"old"), PutMode::Create)
        .await;
    assert_eq!(done.verdict.unwrap().action, Action::Success, "{done:?}");
    let tag = done.value.unwrap().e_tag.unwrap();
    for mode in [
        PutMode::Create,
        PutMode::Update(UpdateVersion {
            e_tag: Some("\"wrong\"".into()),
            version: None,
        }),
    ] {
        let done = probe.put(&key, Bytes::from_static(b"wrong"), mode).await;
        assert_eq!(done.verdict.unwrap().action, Action::ReadBack);
    }
    let done = probe
        .put(
            &key,
            Bytes::from_static(b"new"),
            PutMode::Update(UpdateVersion {
                e_tag: Some(tag),
                version: None,
            }),
        )
        .await;
    assert_eq!(done.verdict.unwrap().action, Action::Success, "{done:?}");
    let read = probe
        .call(Kind::HeadRead, || async {
            probe.store.get(&key).await?.bytes().await
        })
        .await;
    assert_eq!(read.value.unwrap(), b"new"[..]);
}
async fn listing_probe(probe: &Probe, prefix: &str) {
    // Names must be returned in UTF-8 byte order, URL escapes decoded once.
    let mut expected = Vec::new();
    for suffix in ["%value", "#hash", "~tilde", "a space", "é", "漢"] {
        let name = format!("{prefix}order/{suffix}");
        let key = Path::parse(&name).unwrap();
        assert_eq!(
            probe
                .put(&key, Bytes::from_static(b"x"), PutMode::Overwrite)
                .await
                .verdict
                .unwrap()
                .action,
            Action::Success
        );
        expected.push(name);
    }
    expected.sort();
    let folder = Path::parse(format!("{prefix}order/folder/")).unwrap();
    assert_eq!(
        probe
            .put(&folder, Bytes::new(), PutMode::Overwrite)
            .await
            .verdict
            .unwrap()
            .action,
        Action::Success
    );
    let mut got = Vec::new();
    let mut token = None;
    let mut folder_seen = false;
    loop {
        let options = PaginatedListOptions {
            max_keys: Some(2),
            page_token: token.take(),
            ..Default::default()
        };
        let list_prefix = format!("{prefix}order/");
        let done = probe
            .call(Kind::ListPage, || {
                probe
                    .store
                    .list_paginated(Some(&list_prefix), options.clone())
            })
            .await;
        assert_eq!(done.verdict.unwrap().action, Action::Success, "{done:?}");
        let page = done.value.unwrap();
        for object in page.result.objects {
            if object.size == 0 {
                folder_seen = true;
                continue;
            }
            got.push(object.location.to_string());
        }
        token = page.page_token;
        if token.is_none() {
            break;
        }
    }
    assert!(folder_seen);
    assert_eq!(got, expected);
}
async fn multipart_probe(probe: &Probe, prefix: &str) {
    let key = Path::parse(format!("{prefix}multipart.zip")).unwrap();
    let id = probe
        .call(Kind::Create, || probe.store.create_multipart(&key))
        .await
        .value
        .unwrap();
    let mut parts = Vec::new();
    let mut digests = Md5::new();
    for (i, bytes) in [
        Bytes::from(vec![b'a'; 16 * 1024 * 1024]),
        Bytes::from_static(b"tail"),
    ]
    .into_iter()
    .enumerate()
    {
        digests.update(Md5::digest(&bytes));
        let part = probe
            .call(Kind::Part, || {
                probe.store.put_part(&key, &id, i, bytes.clone().into())
            })
            .await;
        assert_eq!(part.verdict.unwrap().action, Action::Success);
        parts.push(part.value.unwrap());
    }
    let expected = format!("{}-2", unityrs_batch::digest::Hex(&digests.finalize()));
    let complete = probe
        .call(Kind::Complete, || {
            probe.store.complete_multipart(&key, &id, parts.clone())
        })
        .await;
    assert_eq!(complete.verdict.unwrap().action, Action::Success);
    assert!(etag::matches(
        complete.value.unwrap().e_tag.unwrap().as_bytes(),
        expected.as_bytes()
    ));
    let abort = probe
        .call(Kind::Abort, || probe.store.abort_multipart(&key, &id))
        .await;
    assert_eq!(abort.verdict.unwrap().action, Action::Success);
    assert!(abort.record.code.is("NoSuchUpload"), "{abort:?}");
}

#[test]
#[ignore = "requires UNITYRS_STORE_SERVER; CI runs it explicitly and cannot skip"]
fn third_party_server_conformance() {
    let binary =
        std::env::var_os("UNITYRS_STORE_SERVER").expect("UNITYRS_STORE_SERVER is required");
    child::scene(|scene| {
        let root=scene.scratch.join("data");std::fs::create_dir_all(root.join("bucket"))?;
        // The pinned server rejects session tokens on long-term root keys.
        // Seed a synthetic temporary identity in its file-backed IAM fixture.
        let iam=scene.scratch.join("iam");std::fs::create_dir(&iam)?;
        let policy=serde_json::json!({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:*","Resource":"*"}]}).to_string();
        let fixture=serde_json::json!({
            "roles":{"probe":{"RoleName":"probe","RoleID":"role-probe","Arn":"arn:aws:iam::000000000000:role/probe","Policies":{"inline":[{"PolicyName":"allow","PolicyDocument":policy}]}}},
            "sessions":{"ASIAPROBE":{"accessKeyId":"ASIAPROBE","secretAccessKey":"session-secret","sessionToken":"synthetic-session-token","roleArn":"arn:aws:iam::000000000000:role/probe","roleName":"probe","roleId":"role-probe","roleSessionName":"probe","createDate":"2026-01-01T00:00:00Z","expiration":"2099-01-01T00:00:00Z"}}
        });
        std::fs::write(iam.join("iam.json"),fixture.to_string())?;
        let socket=scene.scratch.join("iam.sock");
        let mut iam_cmd=Command::new(&binary);
        iam_cmd.env_clear().args(["--access","AKIDEXAMPLE","--secret","secret","--port","127.0.0.1:0","iam","--quiet","--dir"]).arg(&iam).arg("--private-ports").arg(&socket).stdout(Stdio::null()).stderr(Stdio::null());
        let iam_server=child::live(scene,&mut iam_cmd,true)?;
        let until=Instant::now()+DEADLOCK_GUARD;
        while std::os::unix::net::UnixStream::connect(&socket).is_err() {
            assert!(scene.running(iam_server)?,"IAM fixture server exited");
            assert!(Instant::now()<until,"IAM readiness timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0))?;
        let address=listener.local_addr()?;drop(listener);
        let mut cmd=Command::new(&binary);
        cmd.env_clear().args(["--access","AKIDEXAMPLE","--secret","secret","--region","us-east-1","--quiet","--virtual-domain","s3.test","--port"]).arg(address.to_string()).arg("--iam-standalone-endpoint").arg(&socket).arg("--iam-standalone-access").arg("AKIDEXAMPLE").arg("--iam-standalone-secret").arg("secret").arg("posix").arg(&root).stdout(Stdio::null()).stderr(Stdio::null());
        let server=child::live(scene,&mut cmd,true)?;
        let until=Instant::now()+DEADLOCK_GUARD;
        loop {
            if std::net::TcpStream::connect(address).is_ok(){break;}
            assert!(scene.running(server)?,"conformance server exited");
            assert!(Instant::now()<until,"conformance server readiness timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
        store_support::run(async {
            let endpoint=Url::parse(&format!("http://s3.test:{}",address.port())).unwrap();
            // The same signed requests carry an explicit session-token header.
            let credentials=store_support::credentials("ASIAPROBE","session-secret",Some("synthetic-session-token"));
            for style in [true,false] {
                let probe=Probe::new(&endpoint,style,Some(address),&credentials,"bucket","us-east-1");
                probes(&probe,if style {"path/"}else{"virtual/"}).await;
            }
        });
        Ok(())
    }).unwrap();
}

#[test]
#[ignore = "owner-gated provider; explicit endpoint, bucket, prefix and credentials required"]
fn owner_store_probe() {
    let endpoint = Url::parse(
        &std::env::var("UNITYRS_STORE_PROBE_ENDPOINT").expect("probe endpoint required"),
    )
    .unwrap();
    let bucket = std::env::var("UNITYRS_STORE_PROBE_BUCKET").expect("probe bucket required");
    let prefix = std::env::var("UNITYRS_STORE_PROBE_PREFIX").expect("unique probe prefix required");
    let region = std::env::var("UNITYRS_STORE_PROBE_REGION").expect("probe region required");
    assert!(!prefix.is_empty() && prefix.ends_with('/'));
    let credentials = unityrs_batch::store::credentials::Credentials::preflight(None)
        .unwrap()
        .into_provider();
    store_support::run(async {
        for style in [true, false] {
            let probe = Probe::new(&endpoint, style, None, &credentials, &bucket, &region);
            probes(&probe, &format!("{prefix}{style}/")).await;
        }
    });
}
