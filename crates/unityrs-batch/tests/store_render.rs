//! What a hostile store sends back never renders: credentials it reflects
//! in an error body, a 2xx body `object_store` would quote, or a response
//! field reach no error, `Debug`, record or verdict (review round 1).
//!
//! The 2xx bodies `object_store` must parse (a create's, a LIST page's)
//! still reach it, and its parse errors can quote a tag name: the engine
//! formats no `object_store` error, ending each try with an [`Ended`].
mod store_support;
use object_store::{ObjectStoreExt, PutPayload, multipart::MultipartStore, path::Path};
use std::{cell::RefCell, time::Duration};
use store_support::{Harness, Reply, endpoint, run};
use unityrs_batch::store::{
    Kind,
    classify::Ended,
    client::{self, Settings},
    connector::Connector,
};

const KEY: &str = "AKIDREFLECTEDKEY0001";
const SECRET: &str = "reflected/secret+value";
const TOKEN: &str = "reflected-session-token";

/// Asserts no credential appears in `text`.
fn clean(text: &str) {
    for value in [KEY, SECRET, TOKEN] {
        assert!(!text.contains(value), "{value} in {text}");
    }
}

/// Every reply reflects the three values somewhere `object_store` reads.
fn replies() -> Vec<(Kind, Reply)> {
    let echo = format!("{KEY} {SECRET} {TOKEN}");
    let xml = format!("<Error><Code>{KEY}</Code><Message>{echo}</Message></Error>");
    let internal = format!("<Error><Code>InternalError</Code><Message>{echo}</Message></Error>");
    vec![
        (Kind::Put, Reply::status(403, "", xml.as_bytes())),
        (Kind::Put, Reply::status(400, "", xml.as_bytes())),
        (Kind::Abort, Reply::status(404, "", xml.as_bytes())),
        (Kind::Put, Reply::status(500, "", xml.as_bytes())),
        (Kind::Put, Reply::status(503, "", echo.as_bytes())),
        (Kind::Put, Reply::status(404, "", echo.as_bytes())),
        (
            Kind::Put,
            Reply::status(307, &format!("Location: https://x/{TOKEN}\r\n"), b""),
        ),
        (Kind::Put, Reply::status(304, "", echo.as_bytes())),
        (Kind::Complete, Reply::status(200, "", internal.as_bytes())),
        (
            Kind::Complete,
            Reply::status(200, "", format!("<a>SlowDown {echo}</a>").as_bytes()),
        ),
        (
            Kind::Create,
            Reply::status(200, "", format!("<x>{echo}").as_bytes()),
        ),
        (
            Kind::HeadRead,
            Reply::status(200, &format!("Last-Modified: {TOKEN}\r\n"), b"{}"),
        ),
        (
            Kind::HeadRead,
            Reply::status(
                200,
                &format!("ETag: \"e\"\r\nContent-Range: {SECRET}\r\n"),
                b"{}",
            ),
        ),
        (
            Kind::HeadRead,
            Reply::status(206, &format!("Content-Range: bytes {KEY}\r\n"), b"{}"),
        ),
        (
            Kind::HeadRead,
            Reply::status(200, &format!("x-amz-meta-{TOKEN}: {SECRET}\r\n"), b"{}"),
        ),
        (
            Kind::Put,
            Reply::status(
                200,
                &format!("ETag: \"e\"\r\nx-amz-version-id: v\r\nX-{KEY}: 1\r\n"),
                b"",
            ),
        ),
    ]
}

/// One call of `kind`: its error rendered as anything that formatted it
/// would, and its tries' outcome.
fn call(store: &client::Store, kind: Kind, h: &Harness) -> Vec<String> {
    let path = Path::parse("out/a.zip").unwrap();
    let rendered = RefCell::new(Vec::new());
    let done = run(h.tries().run(&h.slot, kind, |_| {
        let (store, path, rendered) = (store, &path, &rendered);
        async move {
            let result: object_store::Result<()> = match kind {
                Kind::Complete => store
                    .complete_multipart(
                        path,
                        &"upload".to_owned(),
                        vec![object_store::multipart::PartId {
                            content_id: "\"p\"".into(),
                        }],
                    )
                    .await
                    .map(drop),
                Kind::Create => store.create_multipart(path).await.map(drop),
                Kind::Abort => store.abort_multipart(path, &"upload".to_owned()).await,
                Kind::HeadRead => match store.get(path).await {
                    Ok(get) => get.bytes().await.map(drop),
                    Err(error) => Err(error),
                },
                _ => store
                    .put(path, PutPayload::from_static(b"x"))
                    .await
                    .map(drop),
            };
            result.map_err(|error| {
                rendered.borrow_mut().push(format!("{error} {error:?}"));
                Ended::Error
            })
        }
    }));
    let mut rendered = rendered.into_inner();
    rendered.push(format!("{done:?}"));
    rendered
}

#[test]
fn reflected_credentials_render_nowhere() {
    let url = endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    for (kind, reply) in replies() {
        let h = Harness::new([]);
        let script = store_support::Script::new([reply]);
        let connector = Connector::new(
            script.clone(),
            client::origin(&settings).unwrap(),
            Duration::from_secs(1),
        );
        let provider = store_support::credentials(KEY, SECRET, Some(TOKEN));
        let store = client::build(&settings, None, &provider, &connector).unwrap();
        let rendered = call(&store, kind, &h);
        // Each call's tries ended, and rendered.
        assert!(
            rendered.iter().any(|text| text.starts_with("Ok(Done")),
            "{rendered:?}"
        );
        for text in rendered {
            clean(&text);
        }
        clean(&format!("{:?} {store:?}", h.slot.record()));
    }
}
