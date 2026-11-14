//! Plan-bounded job envelopes, built during measured phase 1, before the
//! running head. Every job runs once, so its accounts and meter need no reset.
use crate::{budget::SealedBudget, spec::model::Spec};
use std::sync::{Arc, atomic::AtomicU64};
use unityrs_core::{FileContext, RawStr, budget::Reservation, limits::Meter};

pub(crate) struct Memory {
    pub budget: Arc<SealedBudget>,
    pub context: Arc<FileContext>,
    pub meter: Arc<Meter>,
}
impl Memory {
    pub fn new(m: u64, d: u64, input_only: bool, usage: Option<Arc<AtomicU64>>) -> Self {
        let budget = Arc::new(SealedBudget::observed(m, d, input_only, usage));
        let mut context = FileContext::unitypy();
        context.budget = budget.clone();
        context.limits.max_input_memory = m;
        context.limits.max_input_bytes = m;
        context.limits.max_decode_work = m.saturating_mul(4);
        let meter = Meter::for_input(&context.limits, budget.clone(), Reservation::Input);
        Self {
            budget,
            context: Arc::new(context),
            meter,
        }
    }
}
pub(crate) struct Plan {
    pub m: u64,
    pub d: u64,
    /// The UnityCN key preflight read, set in every context below.
    pub key: Option<crate::start::key::UnityCnKey>,
    pub peeks: Vec<Memory>,
    pub attempts: Vec<Memory>,
    pub failures: Vec<Memory>,
    pub names: Vec<RawStr>,
}
impl Plan {
    pub fn new(spec: &Spec) -> Result<Self, crate::phase::Error> {
        Ok(Self {
            m: spec.max_decoded_bytes,
            d: spec.dep_cache_bytes,
            key: None,
            peeks: (0..spec.bundle_count())
                .map(|_| Memory::new(spec.max_decoded_bytes, 0, true, None))
                .collect(),
            attempts: (0..spec.input_count())
                .map(|_| Memory::new(spec.max_decoded_bytes, spec.dep_cache_bytes, false, None))
                .collect(),
            failures: (0..spec.input_count())
                .map(|_| Memory::new(spec.max_decoded_bytes, 0, true, None))
                .collect(),
            names: (0..spec.bundle_count())
                .map(|id| {
                    let path = crate::stage::try_path(
                        std::path::Path::new(""),
                        u32::try_from(id).map_err(|_| crate::phase::Error::internal())?,
                        spec.input_count(),
                        false,
                    )?;
                    let name = RawStr::from(path.as_os_str().as_encoded_bytes());
                    // Bytes promotes a copied slice to shared storage on its
                    // first clone. Reserve that owner during phase 1 too.
                    drop(name.clone());
                    Ok(name)
                })
                .collect::<Result<_, crate::phase::Error>>()?,
        })
    }
    /// Sets `key` in every load's options: before the plan is shared (its
    /// contexts' only owner is this plan), else `Err`.
    pub fn install_key(&mut self, key: crate::start::key::UnityCnKey) -> Result<(), ()> {
        for memory in self
            .peeks
            .iter_mut()
            .chain(&mut self.attempts)
            .chain(&mut self.failures)
        {
            Arc::get_mut(&mut memory.context)
                .ok_or(())?
                .set_assetbundle_decrypt_key(key.bytes())
                .map_err(|_| ())?;
        }
        self.key = Some(key);
        Ok(())
    }
    pub fn observe(&self, usage: &Arc<AtomicU64>) {
        for memory in self
            .peeks
            .iter()
            .chain(&self.attempts)
            .chain(&self.failures)
        {
            memory.budget.observe(usage.clone());
        }
    }
}
