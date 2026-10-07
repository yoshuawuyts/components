use html2md::parse_html;
use pulldown_cmark::{html, Parser};
use readability::extractor;
use url::Url;

wit_bindgen::generate!({
    world: "htmlmark-world"
});

struct Component;

impl exports::htmlmark::Guest for Component {
    fn extract(input: String) -> Result<String, String> {
        let base_url = Url::parse("https://example.com").unwrap();

        match extractor::extract(input.as_bytes(), &base_url) {
            Ok(article) => Ok(parse_html(&article.content)),
            Err(_) => Ok(parse_html(&input)), // fallback
        }
    }

    fn render(md: String) -> Result<String, String> {
        let parser = Parser::new(&md);
        let mut html_output = String::new();
        html::push_html(&mut html_output, parser);
        Ok(html_output)
    }
}
