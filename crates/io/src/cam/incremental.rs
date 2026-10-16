//! Bounded artifact serialization. Large arrays never become an intermediate JSON Value.
use super::{JobInput, Report, Toolpath, escape};
use crate::{IoError, Result};
use serde::Serialize;
use std::future::Future;

const MAX_BYTES: usize = 128 * 1024 * 1024;
struct Writer<C, Y> {
    out: String,
    cancelled: C,
    yield_control: Y,
}
impl<C: Fn() -> bool, Y: FnMut() -> F, F: Future<Output = ()>> Writer<C, Y> {
    async fn checkpoint(&mut self) -> Result<()> {
        if (self.cancelled)() {
            return Err(IoError::Invalid("CAM export cancelled".into()));
        }
        if self.out.len() > MAX_BYTES {
            return Err(IoError::Invalid("CAM artifact exceeds 128 MiB".into()));
        }
        (self.yield_control)().await;
        if (self.cancelled)() {
            return Err(IoError::Invalid("CAM export cancelled".into()));
        }
        Ok(())
    }
    fn value(&mut self, v: &impl Serialize) -> Result<()> {
        self.out.push_str(&serde_json::to_string(v).map_err(|e| IoError::Invalid(e.to_string()))?);
        Ok(())
    }
    async fn array<T: Serialize>(&mut self, values: &[T], batch: usize) -> Result<()> {
        self.out.push('[');
        for (i, value) in values.iter().enumerate() {
            if i.is_multiple_of(batch) {
                self.checkpoint().await?;
            }
            if i != 0 {
                self.out.push(',');
            }
            self.value(value)?;
        }
        self.out.push(']');
        Ok(())
    }
    async fn html_array<T: Serialize>(&mut self, values: &[T]) -> Result<()> {
        self.out.push('[');
        for (i, value) in values.iter().enumerate() {
            self.checkpoint().await?;
            if i != 0 {
                self.out.push(',');
            }
            self.out.push('\n');
            self.out.push_str(&escape(&serde_json::to_string_pretty(value).map_err(|e| IoError::Invalid(e.to_string()))?));
        }
        self.out.push_str("\n]");
        Ok(())
    }
}

/// Build a complete artifact, yielding between bounded array batches and observing cancellation.
/// JSON field values match `artifact`; whitespace/object order may differ. No partial output escapes.
pub async fn artifact_cooperative<F: Future<Output = ()>>(
    input: &JobInput,
    path: &Toolpath,
    report: Option<&Report>,
    format: &str,
    cancelled: impl Fn() -> bool,
    yield_control: impl FnMut() -> F,
) -> Result<String> {
    if format == "stock" {
        return solidcraft_cam::verify::stock_stl_cooperative(
            report.ok_or_else(|| IoError::Invalid("simulate first".into()))?,
            input.setup.stock.min.z,
            cancelled,
            yield_control,
        )
        .await
        .map_err(|e| IoError::Invalid(e.to_string()));
    }
    let mut w = Writer { out: String::new(), cancelled, yield_control };
    w.checkpoint().await?;
    match format {
        "toolpath" => {
            w.out.push_str("{\"version\":");
            w.value(&solidcraft_cam::VERSION)?;
            w.out.push_str(",\"input\":{\"setup\":");
            w.value(&input.setup)?;
            w.out.push_str(",\"tools\":");
            w.array(&input.tools, 1).await?;
            w.out.push_str(",\"operations\":");
            w.array(&input.operations, 1).await?;
            w.out.push_str(",\"fingerprint\":");
            w.value(&input.fingerprint)?;
            w.out.push_str(",\"geometry\":{\"target\":");
            w.array(&input.geometry.target, 256).await?;
            w.out.push_str(",\"fixtures\":");
            w.array(&input.geometry.fixtures, 256).await?;
            w.out.push_str(",\"fixture_groups\":");
            w.array(&input.geometry.fixture_groups, 256).await?;
            w.out.push_str("}},\"toolpath\":{\"fingerprint\":");
            w.value(&path.fingerprint)?;
            w.out.push_str(",\"blocks\":");
            w.array(&path.blocks, 256).await?;
            w.out.push_str("}}");
        }
        "validation" => {
            let r = report.ok_or_else(|| IoError::Invalid("simulate first".into()))?;
            // This small metadata object excludes the potentially large cell array.
            let metadata = serde_json::json!({
                "version":r.version,"fingerprint":r.fingerprint,"status":r.status,
                "finished_dimensions":r.finished_dimensions,"post_version":r.post_version,
                "tolerance":r.tolerance,"uncertainty":r.uncertainty,"maximum_height_interval":r.maximum_height_interval,
                "stock_complete":r.stock_complete,"stock":r.stock,"findings":r.findings,
                "cutting_distance":r.cutting_distance,"rapid_distance":r.rapid_distance,"cutting_minutes":r.cutting_minutes,
                "remaining_volume_low":r.remaining_volume_low,"remaining_volume_high":r.remaining_volume_high,"work":r.work,
            });
            w.value(&metadata)?;
            w.out.pop();
            w.out.push_str(",\"cells\":");
            w.array(&r.cells, 256).await?;
            w.out.push('}');
        }
        "setup_sheet" => {
            w.out = format!(
                "<!doctype html><meta charset=utf-8><title>{}</title><style>body{{font:16px system-ui;max-width:1000px;margin:40px auto}}table{{border-collapse:collapse;width:100%}}td,th{{padding:8px;border:1px solid #ccc;text-align:left}}pre{{white-space:pre-wrap}}</style><h1>{}</h1><p>Reference three-axis milling — not a qualified machine post.</p><p>Fingerprint: {}</p><h2>Setup</h2><pre>{}</pre><h2>Operations</h2><table><tr><th>Name</th><th>Tool</th><th>Feed mm/min</th><th>Spindle rpm</th><th>Bottom mm</th></tr>",
                escape(&input.setup.name),
                escape(&input.setup.name),
                escape(&path.fingerprint),
                escape(&serde_json::to_string_pretty(&input.setup).map_err(|e| IoError::Invalid(e.to_string()))?),
            );
            for o in &input.operations {
                w.checkpoint().await?;
                w.out.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&o.name),
                    o.tool,
                    o.feed,
                    o.spindle,
                    o.bottom
                ));
            }
            w.out.push_str("</table><h2>Tools</h2><pre>");
            w.html_array(&input.tools).await?;
            w.out.push_str("</pre><h2>Operation settings and selected geometry</h2><pre>");
            w.html_array(&input.operations).await?;
            w.out.push_str("</pre>");
        }
        _ => return Err(IoError::Invalid("unknown CAM artifact format".into())),
    }
    w.checkpoint().await?;
    Ok(w.out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::cell::Cell;
    fn ready<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
                return value;
            }
        }
    }
    fn fixture() -> (JobInput, Toolpath, Report) {
        let input = solidcraft_cam::corpus::fixture("face").unwrap();
        let path = solidcraft_cam::generate::generate(input.clone()).unwrap();
        let parsed = solidcraft_cam::nc::round_trip(&path, &input.setup, &input.tools).unwrap();
        let report = solidcraft_cam::verify::simulate(&input, &parsed, Default::default()).unwrap();
        (input, path, report)
    }
    fn pre_values(html: &str) -> Vec<serde_json::Value> {
        html.split("<pre>")
            .skip(1)
            .map(|part| {
                let text = part
                    .split("</pre>")
                    .next()
                    .unwrap()
                    .replace("&lt;", "<")
                    .replace("&gt;", ">")
                    .replace("&quot;", "\"")
                    .replace("&#39;", "'")
                    .replace("&amp;", "&");
                serde_json::from_str(&text).unwrap()
            })
            .collect()
    }
    #[test]
    fn cooperative_artifacts_preserve_every_field_geometry_and_html_escaping() {
        let (mut input, mut path, mut report) = fixture();
        input.operations[0].name = "ø <script>&\"'".into();
        input.setup.name = "stock </pre> &".into();
        path.blocks.extend(std::iter::repeat_n(path.blocks[0].clone(), 1025));
        input.geometry.fixture_groups = vec![input.geometry.target.len()];
        input.geometry.fixtures = input.geometry.target.clone();
        report.findings.push(solidcraft_cam::verify::Finding {
            severity: solidcraft_cam::verify::FindingSeverity::Information,
            kind: "original".into(),
            line: Some(42),
            record: Some(17),
            source: None,
            at: solidcraft_geom::Vec3::new(1., 2., 3.),
            detail: "example".into(),
        });
        for format in ["toolpath", "validation", "setup_sheet", "stock"] {
            let expected = super::super::reference_artifact(&input, &path, Some(&report), format).unwrap();
            let checkpoints = Cell::new(0);
            let actual = ready(artifact_cooperative(
                &input,
                &path,
                Some(&report),
                format,
                || false,
                || {
                    checkpoints.set(checkpoints.get() + 1);
                    std::future::ready(())
                },
            ))
            .unwrap();
            assert!(checkpoints.get() >= 2);
            match format {
                "stock" => assert_eq!(actual, expected),
                "setup_sheet" => {
                    assert_eq!(pre_values(&actual), pre_values(&expected));
                    assert!(!actual.contains("<script>"));
                    assert!(actual.contains("&lt;script&gt;"));
                }
                _ => assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
                    serde_json::from_str::<serde_json::Value>(&expected).unwrap()
                ),
            }
        }
    }
    #[test]
    fn cancellation_at_each_checkpoint_never_returns_a_partial_artifact() {
        let (input, path, report) = fixture();
        for format in ["toolpath", "validation", "setup_sheet", "stock"] {
            let ticks = Cell::new(0);
            ready(artifact_cooperative(
                &input,
                &path,
                Some(&report),
                format,
                || false,
                || {
                    ticks.set(ticks.get() + 1);
                    std::future::ready(())
                },
            ))
            .unwrap();
            for stop in 0..=ticks.get() {
                let current = Cell::new(0);
                let result = ready(artifact_cooperative(
                    &input,
                    &path,
                    Some(&report),
                    format,
                    || current.get() >= stop,
                    || {
                        current.set(current.get() + 1);
                        std::future::ready(())
                    },
                ));
                assert!(result.unwrap_err().to_string().contains("cancelled"), "{format} checkpoint{stop}");
            }
        }
        for format in ["validation", "stock"] {
            assert!(ready(artifact_cooperative(&input, &path, None, format, || false, || std::future::ready(()))).is_err());
        }
        assert!(ready(artifact_cooperative(&input, &path, Some(&report), "unknown", || false, || std::future::ready(()))).is_err());
    }
}
