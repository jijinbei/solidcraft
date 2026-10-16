//! CAM artifact serialization. The engine owns freshness and posting gates.
mod incremental;
pub use incremental::artifact_cooperative;

#[cfg(test)]
use crate::IoError;
use crate::Result;
use solidcraft_cam::{JobInput, Toolpath, verify::Report};
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
/// Synchronous compatibility wrapper; interactive clients use the cooperative entry point.
pub fn artifact(input: &JobInput, path: &Toolpath, report: Option<&Report>, format: &str) -> Result<String> {
    let mut future = std::pin::pin!(artifact_cooperative(input, path, report, format, || false, || std::future::ready(())));
    loop {
        if let std::task::Poll::Ready(result) =
            std::future::Future::poll(future.as_mut(), &mut std::task::Context::from_waker(std::task::Waker::noop()))
        {
            return result;
        }
    }
}

// Original serializer retained only as an independent schema/escaping oracle in tests.
#[cfg(test)]
fn reference_artifact(input: &JobInput, path: &Toolpath, report: Option<&Report>, format: &str) -> Result<String> {
    let json = |v: serde_json::Value| serde_json::to_string_pretty(&v).map_err(|e| IoError::Invalid(e.to_string()));
    match format {
        "toolpath" => json(serde_json::json!({"version":solidcraft_cam::VERSION,"input":input,"toolpath":path})),
        "validation" => json(
            serde_json::to_value(report.ok_or_else(|| IoError::Invalid("simulate first".into()))?).map_err(|e| IoError::Invalid(e.to_string()))?,
        ),
        "stock" => solidcraft_cam::verify::stock_stl(report.ok_or_else(|| IoError::Invalid("simulate first".into()))?, input.setup.stock.min.z)
            .map_err(|e| IoError::Invalid(e.to_string())),
        "setup_sheet" => {
            let mut html = format!(
                "<!doctype html><meta charset=utf-8><title>{}</title><style>body{{font:16px system-ui;max-width:1000px;margin:40px auto}}table{{border-collapse:collapse;width:100%}}td,th{{padding:8px;border:1px solid #ccc;text-align:left}}pre{{white-space:pre-wrap}}</style><h1>{}</h1><p>Reference three-axis milling — not a qualified machine post.</p><p>Fingerprint: {}</p><h2>Setup</h2><pre>{}</pre><h2>Operations</h2><table><tr><th>Name</th><th>Tool</th><th>Feed mm/min</th><th>Spindle rpm</th><th>Bottom mm</th></tr>",
                escape(&input.setup.name),
                escape(&input.setup.name),
                escape(&path.fingerprint),
                escape(&serde_json::to_string_pretty(&input.setup).map_err(|e| IoError::Invalid(e.to_string()))?)
            );
            for o in &input.operations {
                html.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    escape(&o.name),
                    o.tool,
                    o.feed,
                    o.spindle,
                    o.bottom
                ));
            }
            html.push_str("</table><h2>Tools</h2><pre>");
            html.push_str(&escape(&serde_json::to_string_pretty(&input.tools).map_err(|e| IoError::Invalid(e.to_string()))?));
            html.push_str("</pre><h2>Operation settings and selected geometry</h2><pre>");
            html.push_str(&escape(&serde_json::to_string_pretty(&input.operations).map_err(|e| IoError::Invalid(e.to_string()))?));
            html.push_str("</pre>");
            Ok(html)
        }
        _ => Err(IoError::Invalid("unknown CAM artifact format".into())),
    }
}
