use reqwest::Url;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::{collections::HashSet, error::Error, fmt::Display};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
pub struct Story {
    #[serde(alias = "_id")]
    pub id: String,
    story_id: String,
    page: usize,
    uri: String,
    pub story: Vec<String>,
    keywords: Vec<String>,
    title: String,
    author: String,
    #[serde(skip)]
    links: HashSet<String>,
}

impl Story {
    pub fn new() -> Story {
        Default::default()
    }

    pub fn is_empty(&self) -> bool {
        self.story.is_empty()
    }

    pub fn id(&mut self, id: String) -> &Self {
        self.id = id;
        self
    }

    pub fn story_id(&mut self, story_id: String) -> &Self {
        self.story_id = story_id;
        self
    }

    pub fn page(&mut self, page: usize) -> &Self {
        self.page = page;
        self
    }

    pub fn uri(&mut self, uri: String) -> &Self {
        self.uri = uri;
        self
    }

    pub fn get_uri(&self) -> String {
        self.uri.clone()
    }

    pub fn story(&mut self, story: Vec<String>) -> &Self {
        self.story = story;
        self
    }

    pub fn keywords(&mut self, keywords: Vec<String>) -> &Self {
        self.keywords = keywords;
        self
    }

    pub fn title(&mut self, title: String) -> &Self {
        self.title = title;
        self
    }

    pub fn author(&mut self, author: String) -> &Self {
        self.author = author;
        self
    }

    pub fn get_author(&self) -> String {
        self.author.clone()
    }

    pub fn links(&mut self, links: &HashSet<Url>) -> &Self {
        for link in links {
            self.links.insert(link.to_string());
        }
        self
    }
}

impl Story {
    pub fn write(&self, prefix: &str) -> Result<(), Box<dyn Error>> {
        if self.is_empty() {
            Err(Box::new(StringError::new("empty")))
        } else {
            let filename = self.gen_filename(prefix);

            if let Some(r"/") = filename.graphemes(true).last() {
                Err(Box::new(StringError::new("unwritable URI")))
            } else {
                let mut filename = filename;
                filename.push_str(".json");
                log::debug!("{:?} => {:?}", self.uri, filename);
                let filename = std::path::Path::new(&filename);
                std::fs::create_dir_all(filename.parent().unwrap()).unwrap();
                let json = serde_json::to_string(&self).unwrap();
                std::fs::write(filename, json).unwrap();
                Ok(())
            }
        }
    }

    fn gen_filename(&self, prefix: &str) -> String {
        log::debug!("{:?}", self);
        let uri = Url::parse(&self.uri).unwrap();

        let mut filename: String = match uri.host() {
            Some(s) => s.to_string(),
            _ => String::from("unknown"),
        };
        if !self.author.is_empty() {
            filename.push(std::path::MAIN_SEPARATOR);
            filename.push_str(&self.author);
        }
        filename.push_str(uri.path());
        if let Some(query) = uri.query() {
            filename.push('?');
            filename.push_str(query);
        }
        filename.insert_str(0, &format!("{prefix}/"));
        filename
    }
}

#[derive(Debug)]
struct StringError {
    msg: String,
}

impl StringError {
    pub fn new(msg: &str) -> Self {
        Self {
            msg: msg.to_owned(),
        }
    }
}

impl Error for StringError {}

impl Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}
