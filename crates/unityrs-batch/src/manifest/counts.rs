//! Checked row-derived counts. A failed family contributes no partial counts.
use crate::phase::Error;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub bundles: u64,
    pub bundles_failed: u64,
    pub inputs: u64,
    pub inputs_complete: u64,
    pub inputs_with_skips: u64,
    pub inputs_failed: u64,
    pub inputs_not_run: u64,
    pub outputs_written: u64,
    pub outputs_present: u64,
    pub outputs_not_written: u64,
    pub outputs_withheld: u64,
    pub outputs_failed: u64,
    pub outputs_unwritable: u64,
    pub skips: u64,
    pub skips_expected: u64,
    pub collisions: u64,
    pub flips: u64,
    pub archives_written: u64,
    pub archives_present: u64,
}
pub(super) fn increment(n: &mut u64) -> Result<(), Error> {
    *n = n.checked_add(1).ok_or_else(Error::internal)?;
    Ok(())
}
impl Counts {
    pub(super) fn status(&mut self, family: super::Family, status: &str) -> Result<(), Error> {
        use super::Family;
        let target = match (family, status) {
            (Family::Bundles, "staged" | "not_staged") => {
                increment(&mut self.bundles)?;
                return Ok(());
            }
            (Family::Bundles, "failed") => {
                increment(&mut self.bundles)?;
                &mut self.bundles_failed
            }
            (Family::Inputs, status) => {
                increment(&mut self.inputs)?;
                match status {
                    "complete" => &mut self.inputs_complete,
                    "complete_with_skips" => &mut self.inputs_with_skips,
                    "failed" => &mut self.inputs_failed,
                    "not_run" => &mut self.inputs_not_run,
                    _ => return Err(Error::internal()),
                }
            }
            (Family::Outputs, "written") => &mut self.outputs_written,
            (Family::Outputs, "present") => &mut self.outputs_present,
            (Family::Outputs, "not_written") => &mut self.outputs_not_written,
            (Family::Outputs, "withheld") => &mut self.outputs_withheld,
            (Family::Outputs, "failed") => &mut self.outputs_failed,
            (Family::Outputs, "unwritable_key") => &mut self.outputs_unwritable,
            (Family::Archives, "written") => &mut self.archives_written,
            (Family::Archives, "present") => &mut self.archives_present,
            (Family::Archives, "not_written" | "withheld" | "empty" | "failed") => return Ok(()),
            _ => return Err(Error::internal()),
        };
        increment(target)
    }
    pub(super) fn add(&mut self, other: Self) -> Result<(), Error> {
        macro_rules! add { ($($field:ident),+ $(,)?) => { $(self.$field = self.$field.checked_add(other.$field).ok_or_else(Error::internal)?;)+ }; }
        add!(
            bundles,
            bundles_failed,
            inputs,
            inputs_complete,
            inputs_with_skips,
            inputs_failed,
            inputs_not_run,
            outputs_written,
            outputs_present,
            outputs_not_written,
            outputs_withheld,
            outputs_failed,
            outputs_unwritable,
            skips,
            skips_expected,
            collisions,
            flips,
            archives_written,
            archives_present
        );
        Ok(())
    }
}
