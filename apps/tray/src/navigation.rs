//! Native entry points reuse the mounted office, including its retained settings frame.
use tauri::{Url, WebviewWindow};

pub(crate) fn in_place(window: &WebviewWindow, target: &Url) -> bool {
    let showing = window
        .url()
        .is_ok_and(|current| crate::window::ours(&current) && current.path().starts_with("/chat/"));
    if !showing || !(target.path().starts_with("/chat/") || target.path().starts_with("/settings/"))
    {
        return false;
    }
    window.eval(script(target)).is_ok()
}

fn script(target: &Url) -> String {
    let path = serde_json::Value::from(crate::window::destination(target));
    let fallback = if target.path().starts_with("/settings/") {
        format!("location.assign({path});")
    } else if target.query_pairs().any(|(key, _)| key == "check-update") {
        format!("if (AN.updateFlow) AN.updateFlow(); else location.assign({path});")
    } else if target.path() == "/chat/" && target.query().is_none() && target.fragment().is_none() {
        String::new()
    } else {
        format!("if (typeof go === 'function') go({path}); else location.assign({path});")
    };
    format!("if (AN.navigate) AN.navigate({path}); else {{ {fallback} }}")
}
