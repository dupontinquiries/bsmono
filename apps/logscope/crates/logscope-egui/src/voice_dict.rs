//! voice_dict.rs — coder dictation. Turns spoken symbol names into real punctuation so voice dictation
//! produces code/log syntax: "print open paren x close paren semicolon" → `print(x);`. The `spoken_char`
//! vocabulary is ported from voiceterm; `dictate` applies it with code-aware spacing. Pure + unit-tested.

/// Map a spoken symbol NAME (already lowercased; may be one or two words) to its character. Returns None
/// for ordinary words, which the caller keeps verbatim.
pub fn spoken_char(s: &str) -> Option<char> {
    Some(match s.trim() {
        "semicolon" => ';',
        "colon" => ':',
        "comma" => ',',
        "period" | "dot" | "full stop" => '.',
        "open paren" | "open parenthesis" | "left paren" | "paren" => '(',
        "close paren" | "closing paren" | "right paren" | "end paren" => ')',
        "open brace" | "left brace" | "brace" | "curly" => '{',
        "close brace" | "closing brace" | "right brace" => '}',
        "open bracket" | "left bracket" | "bracket" => '[',
        "close bracket" | "closing bracket" | "right bracket" => ']',
        "quote" | "double quote" => '"',
        "single quote" | "apostrophe" | "tick" => '\'',
        "backtick" | "back tick" => '`',
        "equals" | "equal sign" => '=',
        "underscore" => '_',
        "dash" | "hyphen" | "minus" => '-',
        "slash" | "forward slash" => '/',
        "backslash" => '\\',
        "pipe" | "vertical bar" => '|',
        "less than" | "open angle" | "left angle" => '<',
        "greater than" | "close angle" | "right angle" => '>',
        "star" | "asterisk" => '*',
        "at" | "at sign" => '@',
        "hash" | "pound" | "number sign" => '#',
        "dollar" | "dollar sign" => '$',
        "ampersand" | "and sign" => '&',
        "plus" => '+',
        "percent" => '%',
        "caret" => '^',
        "tilde" => '~',
        "bang" | "exclamation" | "exclamation mark" => '!',
        "question mark" => '?',
        _ => return None,
    })
}

/// Transform a dictated utterance into code: replace spoken symbol names (1- or 2-word) with characters,
/// keep other words verbatim, then tighten spacing around brackets, punctuation, `.` and `_`.
pub fn dictate(text: &str) -> String {
    let orig: Vec<&str> = text.split_whitespace().collect();
    let low: Vec<String> = orig.iter().map(|w| w.to_lowercase()).collect();
    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < orig.len() {
        if i + 1 < orig.len() {
            let two = format!("{} {}", low[i], low[i + 1]);
            if let Some(c) = spoken_char(&two) { tokens.push(c.to_string()); i += 2; continue; }
        }
        if let Some(c) = spoken_char(&low[i]) { tokens.push(c.to_string()); i += 1; continue; }
        tokens.push(orig[i].to_string()); // ordinary word: keep original casing
        i += 1;
    }
    fix_spacing(&tokens.join(" "))
}

/// Tighten spacing so the substituted symbols read as code: closers/punctuation hug left, openers hug
/// right, and `.`/`_` hug both sides (method calls, decimals, snake_case identifiers).
fn fix_spacing(s: &str) -> String {
    let mut r = s.to_string();
    for p in [" ;", " ,", " .", " )", " ]", " }", " :", " !", " ?", " _", " (", " ["] {
        r = r.replace(p, &p[1..]); // drop the leading space (call/index/closers/punct hug left)
    }
    for p in ["( ", "[ ", "{ ", ". ", "_ "] {
        r = r.replace(p, &p[..p.len() - 1]); // drop the trailing space
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_and_spacing() {
        assert_eq!(dictate("print open paren x close paren semicolon"), "print(x);");
        assert_eq!(dictate("foo dot bar open paren close paren"), "foo.bar()");
        assert_eq!(dictate("let x equals five"), "let x = five"); // numbers stay words (whisper gives digits)
        assert_eq!(dictate("a less than b"), "a < b");            // operators stay spaced
        assert_eq!(dictate("my underscore var"), "my_var");
        assert_eq!(dictate("open brace close brace"), "{}");
    }

    #[test]
    fn plain_prose_is_untouched() {
        assert_eq!(dictate("the cat sat on the mat"), "the cat sat on the mat");
    }
}
