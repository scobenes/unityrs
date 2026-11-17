//! The operation-to-outcome bridge, over the complete controller and finisher.
use super::*;
use store_support::fake::{Fault, Operation};

#[test]
fn x7_transfer_failures_retries_and_unsettled_tries_reach_the_final_head() {
    store_support::run(async {
        let cases = [
            (
                Fault::error(403, "AccessDenied"),
                false,
                "publish_failed",
                0,
            ),
            (
                Fault::error(403, "ExpiredToken"),
                false,
                "credentials_expired",
                0,
            ),
            (
                Fault::redirect(307, "https://unused.example.test/"),
                false,
                "publish_failed",
                0,
            ),
            (Fault::error(503, "SlowDown"), true, "", 1),
            (
                Fault::error(200, "InternalError"),
                false,
                "publish_mismatch",
                0,
            ),
            (
                Fault {
                    etag: Some("x".repeat(101)),
                    ..Default::default()
                },
                true,
                "",
                1,
            ),
            (
                Fault {
                    lost_response: true,
                    ..Default::default()
                },
                true,
                "",
                1,
            ),
        ];
        for (fault, success, reason, unsettled) in cases {
            let fake = Fake::start().await;
            let scratch = Scratch::new("store-transfer-classification");
            let mut spec = make_spec(&scratch, &fake);
            pixel(&scratch);
            spec["sink"]["attempts"] = 2.into();
            spec["http"] = json!({"backoff_initial_ms":0,"backoff_max_ms":0});
            fake.schedule(Operation::Put, "p/a/pixel.png", 1, fault);
            let (end, _) = execute(spec, scratch).await;
            assert_eq!(
                end.code,
                if success {
                    Code::Complete
                } else {
                    Code::Incomplete
                }
            );
            let head = head(&fake);
            assert_eq!(head["unsettled_tries"], unsettled);
            assert_eq!(
                head["verified"],
                if success {
                    json!(true)
                } else if reason == "publish_mismatch" {
                    json!(false)
                } else {
                    Value::Null
                }
            );
            let row = fake
                .object("bucket", "p/_unityrs/r/1/outputs.0.ndjson")
                .unwrap();
            let row: Value = serde_json::from_slice(&row.bytes).unwrap();
            assert_eq!(row["status"], if success { "written" } else { "failed" });
            assert_eq!(
                row["reason"],
                if success { Value::Null } else { json!(reason) }
            );
            let puts = fake
                .logs()
                .iter()
                .filter(|l| l.operation == Operation::Put && l.raw_path == "/bucket/p/a/pixel.png")
                .count();
            assert_eq!(puts, if success { 2 } else { 1 });
            assert!(fake.logs().iter().all(|l| l.signature_valid));
        }
    });
}
#[test]
fn x8_lost_running_and_final_responses_reconcile_the_same_write_sequences() {
    store_support::run(async {
        let fake = Fake::start().await;
        let scratch = Scratch::new("store-head-recovery");
        let mut spec = make_spec(&scratch, &fake);
        spec["http"] = json!({"backoff_initial_ms":0,"backoff_max_ms":0});
        for ordinal in [1, 3] {
            fake.schedule(
                Operation::Put,
                "p/_unityrs/r/manifest.json",
                ordinal,
                Fault {
                    lost_response: true,
                    ..Default::default()
                },
            );
        }
        let (end, _) = execute(spec, scratch).await;
        assert_eq!(end.code, Code::Complete);
        assert_eq!(head(&fake)["write_seq"], 1);
        let heads = fake
            .logs()
            .into_iter()
            .filter(|l| l.raw_path == "/bucket/p/_unityrs/r/manifest.json")
            .collect::<Vec<_>>();
        assert_eq!(
            heads
                .iter()
                .filter(|l| l.operation == Operation::Put)
                .count(),
            4
        );
        assert_eq!(
            heads
                .iter()
                .filter(|l| l.operation == Operation::Get)
                .count(),
            3
        );
        assert!(
            heads
                .iter()
                .filter(|l| l.operation == Operation::Put)
                .all(|l| l
                    .headers
                    .iter()
                    .any(|(name, value)| name == "if-match"
                        || (name == "if-none-match" && value == "*")))
        );
    });
}
