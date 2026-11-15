//! The raw audio target uses the same synthetic builders as its harness tests.
pub(super) fn write(s: &mut super::Seeds) -> std::io::Result<()> {
    unityrs_testkit::fsb5::seeds(|name, bytes| s.add("fsb5", name, bytes))
}
