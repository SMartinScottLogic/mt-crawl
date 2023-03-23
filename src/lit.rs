use itertools::Itertools;
use regex::Regex;
use reqwest::blocking::Response;
use reqwest::Url;
use select::document::Document;
use select::predicate::{And, Attr, Class, Name};

use std::collections::{HashMap, HashSet};

pub struct Lit {}

const ANCHOR_TAG: &str = "a";
const BASE: &str = "base";
const HEAD: &str = "head";
const HREF: &str = "href";
const META_TAG: &str = "meta";
const NAME_ATTR: &str = "name";
const KEYWORDS: &str = "keywords";
const CONTENT: &str = "content";

impl Default for Lit {
    fn default() -> Self {
        Self::new()
    }
}

impl Lit {
    pub fn new() -> Lit {
        Lit {}
    }

    pub fn process(
        &self,
        uri: &str,
        config: &crate::CrawlConfig,
        resp: Response,
    ) -> Result<(crate::Story, HashSet<(Url, bool)>), std::io::Error> {
        let document = Document::from_read(resp)?;
        let title = Lit::title(&document);

        let base: String = document
            .find(Name(HEAD))
            .map(|node| {
                node.find(And(Name(BASE), Attr(HREF, ())))
                    .map(|base| base.attr(HREF).unwrap().to_string())
                    .next()
                    .unwrap_or_else(|| uri.to_owned())
            })
            .next()
            .unwrap_or_else(|| uri.to_owned());
        let base = Url::parse(&base).unwrap();
        debug!("base: {}", base);

        let author = config
            .rules
            .author
            .iter()
            .flat_map(|rule| {
                document
                    .find(And(Name(rule.name.as_str()), Class(rule.class.as_str())))
                    .map(|node| node.text())
            })
            .unique()
            .join(" ")
            .trim()
            .to_string();
        debug!("author: {}", author);

        let keywords = document
            .find(Name(HEAD))
            .map(|node| {
                node.find(And(Name(META_TAG), Attr(NAME_ATTR, KEYWORDS)))
                    .map(|node| node.attr(CONTENT).unwrap())
                    .join(" ")
            })
            .join(" ")
            .split(',')
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>();
        debug!("keywords: {:?}", keywords);
        let all_links = document
            .find(And(Name(ANCHOR_TAG), Attr(HREF, ())))
            .map(|a| a.attr(HREF).unwrap().to_string())
            .filter_map(|link| base.join(&link).ok())
            .map(|link| {
                let query: Vec<(_, _)> = link
                    .query_pairs()
                    .filter(|(key, _value)| key != "comments_after")
                    .collect();
                let mut link2 = link.clone();
                link2.set_fragment(None);
                link2.set_query(None);

                for (key, value) in query {
                    link2.query_pairs_mut().append_pair(&key, &value);
                }
                link2
            })
            .filter(|link| Lit::permitted(link, config))
            .collect::<HashSet<_>>();
        let parsed_url = Url::parse(uri).unwrap();
        let query = parsed_url.query_pairs().collect::<HashMap<_, _>>();

        let page = query
            .get("page")
            .unwrap_or(&std::borrow::Cow::from(""))
            .parse()
            .unwrap_or(1);

        let mut story = crate::Story::new();
        story.id(uri.to_string());
        story.story_id(parsed_url.path().to_string());
        story.page(page);
        story.uri(uri.to_string());
        story.story(Lit::story(&document, config, "p"));
        story.keywords(keywords);
        story.title(title);
        story.author(author);
        story.links(&all_links);

        info!("SUCCESS {}", uri.to_string());

        Ok((
            story,
            all_links
                .iter()
                .map(|link| (link.clone(), Lit::is_priority(&parsed_url, link)))
                .collect(),
        ))
    }

    fn is_priority(source: &Url, target: &Url) -> bool {
        source.path().starts_with("/s/") && source.path() == target.path()
    }

    fn story(document: &Document, config: &crate::CrawlConfig, inner_type: &str) -> Vec<String> {
        config
            .rules
            .story
            .iter()
            .flat_map(|rule| {
                document
                    .find(And(Name(rule.name.as_str()), Class(rule.class.as_str())))
                    .flat_map(|node| {
                        node.find(Name(inner_type))
                            .map(|p| p.text())
                            .flat_map(|p| {
                                p.lines()
                                    .map(|l| l.to_string())
                                    .filter(|l| !l.is_empty())
                                    .collect::<Vec<_>>()
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
    }

    fn title(document: &Document) -> String {
        document
            .find(Name("head"))
            .map(|node| {
                node.find(Name("title"))
                    .map(|node| node.text())
                    .fold(String::new(), |acc, text| acc + " " + &text)
            })
            .join(" ")
            .trim()
            .to_string()
    }

    fn permitted_path(path: &str, config: &crate::CrawlConfig) -> Option<bool> {
        let mut permit = Some(false);
        for (k, v) in &config.path_patterns {
            if Regex::new(k).unwrap().is_match(path) {
                trace!("{} matches {}", k, path);
                permit = match v {
                    Some(true) => Some(true),
                    Some(false) | None => permit,
                }
            }
        }
        debug!("{} {:?}", path, permit);
        permit
    }

    fn permitted(parsed_url: &Url, config: &crate::CrawlConfig) -> bool {
        let permitted_host = match parsed_url.host_str() {
            Some(host) if config.hosts.contains_key(host) => config.hosts.get(host).unwrap(),
            _ => &Some(false),
        };
        let permitted = permitted_host.or_else(|| Lit::permitted_path(parsed_url.path(), config));
        debug!(
            "permitted: {:?} {:?} {:?}",
            permitted, permitted_host, parsed_url
        );
        permitted.unwrap_or(false)
    }
}
