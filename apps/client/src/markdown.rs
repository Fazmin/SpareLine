//! A tiny, safe inline formatter: `**bold**`, `*italic*`/`_italic_`, `` `code` `` and
//! web links. It produces text nodes and elements only; message text is never parsed as HTML.

use leptos::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Text(String),
    Bold(String),
    Italic(String),
    Code(String),
    Link(String),
}

fn take_delim(chars: &[char], i: usize, delim: &[char]) -> Option<(String, usize)> {
    let n = delim.len();
    if chars.len() < i + n * 2 + 1 || chars[i..i + n] != *delim {
        return None;
    }
    // No space right after the opening delimiter.
    if chars[i + n].is_whitespace() {
        return None;
    }
    let mut j = i + n;
    while j + n <= chars.len() {
        if chars[j..j + n] == *delim && !chars[j - 1].is_whitespace() && j > i + n {
            let inner: String = chars[i + n..j].iter().collect();
            if inner.contains('\n') {
                return None;
            }
            return Some((inner, j + n));
        }
        j += 1;
    }
    None
}

pub fn parse(text: &str) -> Vec<Seg> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Seg> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let flush = |buf: &mut String, out: &mut Vec<Seg>| {
        if !buf.is_empty() {
            out.push(Seg::Text(std::mem::take(buf)));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        // Links.
        if (c == 'h' || c == 'H') && (i == 0 || !chars[i - 1].is_alphanumeric()) {
            let rest: String = chars[i..chars.len().min(i + 8)]
                .iter()
                .collect::<String>()
                .to_lowercase();
            if rest.starts_with("https://") || rest.starts_with("http://") {
                let mut j = i;
                while j < chars.len()
                    && !chars[j].is_whitespace()
                    && chars[j] != '<'
                    && chars[j] != '>'
                {
                    j += 1;
                }
                while j > i
                    && matches!(
                        chars[j - 1],
                        '.' | ',' | ';' | ':' | '!' | '?' | ')' | '"' | '\''
                    )
                {
                    j -= 1;
                }
                let url: String = chars[i..j].iter().collect();
                if url.len() > 10 {
                    flush(&mut buf, &mut out);
                    out.push(Seg::Link(url));
                    i = j;
                    continue;
                }
            }
        }
        if c == '`'
            && let Some(end) = chars[i + 1..].iter().position(|x| *x == '`')
            && end > 0
        {
            flush(&mut buf, &mut out);
            out.push(Seg::Code(chars[i + 1..i + 1 + end].iter().collect()));
            i += end + 2;
            continue;
        }
        if c == '*'
            && let Some((inner, next)) = take_delim(&chars, i, &['*', '*'])
        {
            flush(&mut buf, &mut out);
            out.push(Seg::Bold(inner));
            i = next;
            continue;
        }
        let word_start = i == 0 || !chars[i - 1].is_alphanumeric();
        if (c == '*' || c == '_')
            && word_start
            && let Some((inner, next)) = take_delim(&chars, i, &[c])
            && (next >= chars.len() || !chars[next].is_alphanumeric())
        {
            flush(&mut buf, &mut out);
            out.push(Seg::Italic(inner));
            i = next;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut out);
    out
}

/// Render formatted text. `on_link` is called with the URL when a link is activated.
pub fn render(text: &str, on_link: Callback<String>) -> impl IntoView {
    parse(text)
        .into_iter()
        .map(|s| match s {
            Seg::Text(t) => t.into_any(),
            Seg::Bold(t) => view! { <strong>{t}</strong> }.into_any(),
            Seg::Italic(t) => view! { <em>{t}</em> }.into_any(),
            Seg::Code(t) => view! { <code>{t}</code> }.into_any(),
            Seg::Link(url) => {
                let u = url.clone();
                let href = url.clone();
                view! {
                    <a
                        href=href
                        on:click=move |e| {
                            e.prevent_default();
                            on_link.run(u.clone());
                        }
                    >
                        {url}
                    </a>
                }
                .into_any()
            }
        })
        .collect_view()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_inline_formatting() {
        assert_eq!(
            parse("see **this** and `code` at https://example.com/x."),
            vec![
                Seg::Text("see ".into()),
                Seg::Bold("this".into()),
                Seg::Text(" and ".into()),
                Seg::Code("code".into()),
                Seg::Text(" at ".into()),
                Seg::Link("https://example.com/x".into()),
                Seg::Text(".".into()),
            ]
        );
        assert_eq!(parse("2*3*4"), vec![Seg::Text("2*3*4".into())]);
        assert_eq!(parse("<b>x</b>"), vec![Seg::Text("<b>x</b>".into())]);
    }
}
