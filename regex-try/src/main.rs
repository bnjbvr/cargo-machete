use dioxus::prelude::*;
use grep::{
    regex::RegexMatcherBuilder,
    searcher::{self, BinaryDetection, SearcherBuilder},
};

fn main() {
    dioxus_desktop::launch(app);
}

fn search(pattern: &str, content: &str) -> anyhow::Result<bool> {
    let matcher = RegexMatcherBuilder::new()
        .multi_line(true)
        .build(&pattern)?;

    let mut searcher = SearcherBuilder::new()
        .binary_detection(BinaryDetection::quit(b'\x00'))
        .multi_line(true)
        .line_number(true)
        .build();

    let mut found = false;
    let mut sink = searcher::sinks::UTF8(|_, _| {
        found = true;
        // Abort search.
        Ok(false)
    });

    searcher
        .search_reader(&matcher, content.as_bytes(), &mut sink)
        .map_err(|err| anyhow::anyhow!("when searching: {}", err))
        .map(|_| found)
}

fn app(cx: Scope) -> Element {
    let regexp = use_state(&cx, || String::new());
    let content = use_state(&cx, || String::new());

    let result = match search(&regexp, &content) {
        Ok(found) => found.to_string(),
        Err(err) => {
            format!("error when executing regexp: {}", err)
        }
    };

    cx.render(rsx! (
        div {
            h1 { "Try your regular expressions!" }

            p { "Regular expression:" }
            textarea {
                oninput: move |text| {
                    regexp.set(text.value.clone());
                },
            }

            p { "Text to be matched:" }
            textarea {
                oninput: move |text |{
                    content.set(text.value.clone())
                },
            }

            p { "Result: {result}" }
        }
    ))
}
