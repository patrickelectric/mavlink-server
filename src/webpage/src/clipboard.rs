//! Clipboard helper that works in both secure and insecure (plain HTTP) browser contexts.
//!
//! `navigator.clipboard` is only available in secure contexts (HTTPS or localhost). Since
//! mavlink-server is commonly reached over plain HTTP on a LAN IP, we cannot rely on it alone.
//! This helper prefers the async Clipboard API when available and falls back to a hidden
//! `<textarea>` + `document.execCommand('copy')`, guarding everything so it can never throw.

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function copy_text(text) {
    try {
        if (navigator.clipboard && window.isSecureContext) {
            navigator.clipboard.writeText(text).catch(function (err) {
                console.error("navigator.clipboard.writeText failed:", err);
            });
            return;
        }

        const textarea = document.createElement("textarea");
        textarea.value = text;
        textarea.setAttribute("readonly", "");
        textarea.style.position = "fixed";
        textarea.style.top = "-1000px";
        textarea.style.opacity = "0";
        document.body.appendChild(textarea);
        textarea.focus();
        textarea.select();

        let ok = false;
        try {
            ok = document.execCommand("copy");
        } catch (err) {
            console.error("document.execCommand('copy') failed:", err);
        }

        document.body.removeChild(textarea);

        if (!ok) {
            console.error("Failed to copy text to clipboard");
        }
    } catch (err) {
        console.error("copy_text failed:", err);
    }
}
"#)]
extern "C" {
    fn copy_text(text: &str);
}

/// Copies `text` to the system clipboard. No-op on non-wasm targets.
pub fn copy_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    copy_text(text);

    #[cfg(not(target_arch = "wasm32"))]
    let _ = text;
}
