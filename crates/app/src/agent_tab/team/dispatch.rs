use nmt_agent::team::model::ContextLimits;

pub(super) const CONTEXT_LIMITS: ContextLimits = ContextLimits {
    max_bytes: 96_000,
    recent_messages: 6,
};
