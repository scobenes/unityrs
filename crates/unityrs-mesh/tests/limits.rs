use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use unityrs_core::LimitKind;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_mesh::{MeshError, read_mesh};
use unityrs_testkit::ngui::{self, V, set_field, st, vector};
use unityrs_testkit::scene3d::mesh::{self, MeshVersion};
mod support;

fn charge(e: &MeshError) -> Option<&ChargeError> {
    match e {
        MeshError::Charge(e) | MeshError::Decode(unityrs_meshdecode::MeshError::Charge(e)) => {
            Some(e)
        }
        _ => None,
    }
}
#[derive(Debug)]
struct Refuse {
    next: AtomicUsize,
    at: usize,
}
impl MemoryBudget for Refuse {
    fn try_reserve(&self, requested: u64, what: Reservation) -> Result<Permit, BudgetExceeded> {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        if n >= self.at {
            Err(BudgetExceeded { requested, what })
        } else {
            Ok(Permit::detached(requested))
        }
    }
}
#[test]
fn memory_work_and_budget_refusals_return_all_charges() {
    let (env, file) = support::load(MeshVersion::V6000, &[support::rich_object(96)]);
    let object = file.object(1).unwrap();
    let full = Meter::unlimited();
    let m = read_mesh(&object, &env, &full).unwrap();
    let used = full.used();
    let work = full.decoded();
    assert!(used > 30_000 && work > used);
    drop(m);
    assert_eq!(full.used(), 0);
    for bytes in [0, 64, 1024, 4096, 16_384, used - 1] {
        let meter = Meter::new(bytes, Arc::new(Unbounded), Reservation::Input);
        let e = read_mesh(&object, &env, &meter).unwrap_err();
        assert!(
            matches!(charge(&e),Some(ChargeError::Limit(l)) if l.kind()==LimitKind::InputMemory),
            "{bytes}: {e:?}"
        );
        assert_eq!(meter.used(), 0);
    }
    for work in [0, 64, 1024, 16_384, work - 1] {
        let meter =
            Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
        let e = read_mesh(&object, &env, &meter).unwrap_err();
        assert!(
            matches!(charge(&e),Some(ChargeError::Limit(l)) if l.kind()==LimitKind::DecodeWork),
            "{work}: {e:?}"
        );
        assert_eq!(meter.used(), 0);
    }
    // Refuse every reservation in turn, including every nested channel name
    // and LOD-level buffer, with the dependency tag preserved.
    let mut success = false;
    for at in 0..1024 {
        let budget = Arc::new(Refuse {
            next: AtomicUsize::new(0),
            at,
        });
        let meter = Meter::new(u64::MAX, budget, Reservation::Dependency);
        match read_mesh(&object, &env, &meter) {
            Ok(m) => {
                drop(m);
                success = true;
                assert!(at > 100);
            }
            Err(e) => assert!(
                matches!(charge(&e),Some(ChargeError::Budget(b)) if b.what==Reservation::Dependency),
                "{at}: {e:?}"
            ),
        }
        assert_eq!(meter.used(), 0, "{at}");
        if success {
            break;
        }
    }
    assert!(success);
}

#[test]
fn malformed_metadata_and_wrong_objects_refuse_without_panicking() {
    let cases = [
        ("m_Name", V::UInt(1)),
        ("m_BindPose", V::UInt(1)),
        ("m_BoneNameHashes", vector(V::Int(0), vec![V::Int(-1)])),
        ("m_RootBoneNameHash", V::UInt64(u64::MAX)),
        ("m_LocalAABB", st("AABB", vec![("m_Center", V::UInt(3))])),
        ("m_Shapes", V::UInt(1)),
        (
            "m_MeshLodInfo",
            mesh::lod_info(1, 1.0, 1.0, &[vec![(0, 3)]]),
        ),
    ];
    for (field, value) in cases {
        let mut obj = set_field(
            mesh::mesh(
                1,
                b"malformed",
                &[[0.0; 3]; 3],
                &[0, 1, 2],
                MeshVersion::V2022,
            ),
            field,
            value,
            None,
        );
        if field == "m_MeshLodInfo" {
            obj = set_field(
                obj,
                field,
                st("MeshLodInfo", vec![("m_NumLevels", V::Int(-1))]),
                None,
            );
        }
        let (env, file) = support::load(MeshVersion::V2022, &[obj]);
        let meter = Meter::unlimited();
        assert!(
            read_mesh(&file.object(1).unwrap(), &env, &meter).is_err(),
            "{field}"
        );
        assert_eq!(meter.used(), 0);
    }
    let object = ngui::Object::new(1, 21, st("Material", vec![]));
    let (env, file) = support::load(MeshVersion::V2022, &[object]);
    assert!(matches!(
        read_mesh(&file.object(1).unwrap(), &env, &Meter::unlimited()),
        Err(MeshError::NotMesh)
    ));
}

#[test]
fn seeded_mutations_never_panic_or_keep_a_failed_models_charge() {
    use unityrs_core::env::{EnvOptions, Environment, Source};
    use unityrs_core::{Bytes, File, RawStr};
    let object = support::rich_object(6);
    let mut writer = unityrs_testkit::ByteWriter::le();
    ngui::write(&mut writer, &object.value).unwrap();
    let original = writer.into_inner();
    let nodes = ngui::nodes(0, "Base", &object.value).unwrap();
    let mut rng = unityrs_testkit::Rng::new(0x6d65_7368_0202);
    for i in 0..256 {
        let mut data = original.clone();
        rng.flip_bits(&mut data, 1 + i % 4);
        if i % 3 == 0 {
            rng.truncate(&mut data);
        }
        let bytes = unityrs_testkit::serialized::build(&unityrs_testkit::serialized::one_object(
            43,
            nodes.clone(),
            data,
        ));
        let mut options = EnvOptions::unitypy();
        let mut context = (*options.context).clone();
        context.limits.max_input_memory = 16 << 20;
        context.limits.max_decode_work = 16 << 20;
        options.context = Arc::new(context);
        let env = Environment::new(options);
        if let Ok(File::Serialized(file)) = env.load(Source::Bytes {
            name: RawStr::from("mutant.assets"),
            data: Bytes::from(bytes),
        }) {
            let meter = Meter::with_decode_work(
                16 << 20,
                16 << 20,
                Arc::new(Unbounded),
                Reservation::Input,
            );
            if let Some(object) = file.object(1) {
                drop(read_mesh(&object, &env, &meter));
            }
            assert_eq!(meter.used(), 0);
        }
    }
}
