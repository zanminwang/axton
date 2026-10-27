//! Opt-in, instance-local tracing collection. Never installs a global subscriber.
//! Records are measured here, not when a language bridge eventually drains them.
use serde_json::{Map, Value, json};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::{
    Id, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Record},
};
use tracing_subscriber::{Layer, Registry, layer::Context, prelude::*, registry::LookupSpan};

pub(crate) const CAPACITY: usize = 4096;

#[derive(Clone)]
pub struct Diagnostics {
    inner: Arc<Inner>,
}
struct Inner {
    origin: Instant,
    clock: String,
    records: Mutex<(Vec<Value>, u64)>,
}
struct SpanData {
    start: Instant,
    offset: u64,
    parent: Option<u64>,
    fields: Fields,
}
#[derive(Default)]
struct Fields(Map<String, Value>);
impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
}
impl Diagnostics {
    pub fn new(clock: String) -> Self {
        Self {
            inner: Arc::new(Inner {
                origin: Instant::now(),
                clock,
                records: Mutex::new((Vec::new(), 0)),
            }),
        }
    }
    pub fn dispatch(&self) -> tracing::Dispatch {
        tracing::Dispatch::new(Registry::default().with(self.clone()))
    }
    fn push(&self, record: Value) {
        let mut pending = self.inner.records.lock().unwrap_or_else(|e| e.into_inner());
        if pending.0.len() < CAPACITY {
            pending.0.push(record);
        } else {
            pending.1 += 1;
        }
    }
    /// Bounded per collector, including explicit loss accounting.
    pub fn take(&self) -> (Vec<Value>, u64) {
        std::mem::take(&mut *self.inner.records.lock().unwrap_or_else(|e| e.into_inner()))
    }
}
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Diagnostics {
    fn enabled(&self, metadata: &tracing::Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        metadata.target().starts_with("axton::")
    }
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let start = Instant::now();
            let mut fields = Fields::default();
            attrs.record(&mut fields);
            let parent = span.parent().map(|p| p.id().into_u64());
            span.extensions_mut().insert(SpanData {
                start,
                offset: start.duration_since(self.inner.origin).as_micros() as u64,
                parent,
                fields,
            });
        }
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && let Some(data) = span.extensions_mut().get_mut::<SpanData>()
        {
            values.record(&mut data.fields);
        }
    }
    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id)
            && let Some(data) = span.extensions_mut().remove::<SpanData>()
        {
            self.push(json!({"schemaVersion":1,"kind":"span","name":span.name(),"source":"rust","clockDomain":self.inner.clock,"spanId":id.into_u64().to_string(),"parentSpanId":data.parent.map(|p|p.to_string()),"startUs":data.offset,"durationUs":data.start.elapsed().as_micros() as u64,"attributes":data.fields.0}));
        }
    }
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.push(json!({"schemaVersion":1,"kind":"event","name":event.metadata().name(),"source":"rust","clockDomain":self.inner.clock,"startUs":self.inner.origin.elapsed().as_micros() as u64,"attributes":fields.0}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn collectors_are_isolated_and_keep_parentage_and_fields() {
        let a = Diagnostics::new("a".into());
        let b = Diagnostics::new("b".into());
        tracing::dispatcher::with_default(&a.dispatch(), || {
            let root = tracing::info_span!(target: "axton::test", "root");
            let _entered = root.enter();
            let _child = tracing::info_span!(target: "axton::test", "child", rows = 100_u64);
        });
        let (records, dropped) = a.take();
        assert_eq!(dropped, 0);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["parentSpanId"], records[1]["spanId"]);
        assert_eq!(records[0]["attributes"]["rows"], 100);
        assert!(b.take().0.is_empty());
    }
    #[test]
    fn bounded_buffer_reports_loss() {
        let collector = Diagnostics::new("test".into());
        tracing::dispatcher::with_default(&collector.dispatch(), || {
            for _ in 0..CAPACITY + 3 {
                tracing::info!(target: "axton::test", rows = 1_u64);
            }
        });
        let (records, dropped) = collector.take();
        assert_eq!(records.len(), CAPACITY);
        assert_eq!(dropped, 3);
        assert_eq!(collector.take(), (vec![], 0));
    }
}
