//! Native entry points reuse the mounted office, including its retained settings frame.
use tauri::{Url, WebviewWindow};

pub(crate) fn in_place(window: &WebviewWindow, target: &Url) -> bool {
    let showing = window
        .url()
        .is_ok_and(|current| crate::window::ours(&current) && current.path().starts_with("/chat/"));
    if !showing
        || !(target.path().starts_with("/chat/")
            || target.path().starts_with("/settings/")
            || target.path().starts_with("/usage/"))
    {
        return false;
    }
    window.eval(script(target)).is_ok()
}

fn script(target: &Url) -> String {
    let path = serde_json::Value::from(crate::window::destination(target));
    let fallback = if target.path().starts_with("/settings/")
        || target.path().starts_with("/usage/")
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_navigation_uses_the_office_and_has_a_direct_fallback() {
        let target = Url::parse("agentnave://localhost/usage/?range=7d").unwrap();
        let script = script(&target);
        assert!(script.contains("AN.navigate"));
        assert!(script.contains("location.assign"));
        assert!(script.contains("/usage/?range=7d"));
    }
}
