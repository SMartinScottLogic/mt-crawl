#![feature(proc_macro_hygiene, decl_macro)]
#[macro_use] extern crate rocket;

use chrono::{Datelike, Utc};
use crossbeam::channel::{Receiver, Sender};
use crossbeam::deque::{
    Injector, Steal,
    Steal::{Empty, Retry, Success},
};
use crossbeam::scope;
use mt_crawl::Story;
use reqwest::blocking::Client;
use reqwest::Url;
use serde_derive::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::io::Read;
use std::str::FromStr;
use std::sync::Arc;
use std::thread;

#[macro_use]
extern crate log;
#[macro_use]
extern crate lazy_static;

mod lit;
use crate::lit::Lit;

lazy_static! {
    static ref START_DATE: String = {
        let now = Utc::now();
        format!("{:04}.{:02}.{:02}", now.year(), now.month(), now.day())
    };
}

#[derive(Default, Serialize, Deserialize, Debug)]
struct ParseRule {
    name: String,
    class: String,
}

#[derive(Default, Serialize, Deserialize, Debug)]
struct ParseRules {
    author: Vec<ParseRule>,
    story: Vec<ParseRule>,
}

#[derive(Default, Serialize, Deserialize, Debug)]
pub struct CrawlConfig {
    threads: usize,
    seeds: Vec<String>,
    rules: ParseRules,
    hosts: HashMap<String, Option<bool>>,
    path_patterns: HashMap<String, Option<bool>>,
}

trait KnownURL {
    fn insert(&mut self, value: &str);
    fn contains(&self, value: &str) -> bool;
}

#[derive(Debug, Default)]
struct KnownURLSet {
    known_urls: HashSet<String>,
}

impl KnownURL for KnownURLSet {
    fn insert(&mut self, value: &str) {
        self.known_urls.insert(value.to_string());
    }

    fn contains(&self, value: &str) -> bool {
        self.known_urls.contains(value)
    }
}

#[derive(Debug, Default)]
struct KnownURLHashSet {
    known_url_hashes: HashSet<u64>,
}

impl KnownURL for KnownURLHashSet {
    fn insert(&mut self, value: &str) {
        let mut hasher = DefaultHasher::new();
        hasher.write(value.as_bytes());
        let hash = hasher.finish();

        self.known_url_hashes.insert(hash);
    }

    fn contains(&self, value: &str) -> bool {
        let mut hasher = DefaultHasher::new();
        hasher.write(value.as_bytes());
        let hash = hasher.finish();
        self.known_url_hashes.contains(&hash)
    }
}

struct PriorityInjector<T> {
    priority_q: Injector<T>,
    q: Injector<T>,
}

impl<T> PriorityInjector<T> {
    pub fn new() -> Self {
        Self {
            priority_q: Injector::new(),
            q: Injector::new(),
        }
    }

    pub fn push(&self, value: T, priority: bool) {
        match priority {
            true => self.priority_q.push(value),
            false => self.q.push(value),
        }
    }

    pub fn steal(&self) -> Steal<T> {
        if self.priority_q.is_empty() {
            debug!("claim from default queue");
            self.q.steal()
        } else {
            debug!("claim from priority queue");
            self.priority_q.steal()
        }
    }
}

fn run_thread(
    q: &PriorityInjector<String>,
    s: Sender<(Story, HashSet<(Url, bool)>)>,
    config: Arc<CrawlConfig>,
) {
    let client = Client::new();
    let lit = Lit::new();
    loop {
        match q.steal() {
            Success(uri) => {
                debug!(
                    "{}: received {}",
                    thread::current().name().unwrap_or("None"),
                    uri
                );
                match client.get(&uri).send() {
                    Ok(resp) => match lit.process(&uri, &config, resp) {
                        Ok(data) => s.send(data).unwrap(),
                        Err(e) => error!("{} => {}", uri, e),
                    },
                    Err(e) => error!("{uri} => {e}"),
                }
            }
            Retry => (),
            Empty => (),
        }
    }
    info!("{}: done", thread::current().name().unwrap_or("None"));
}

fn receiver(
    r: Receiver<(Story, HashSet<(Url, bool)>)>,
    q: &PriorityInjector<String>,
    mut known_urls: impl KnownURL,
) {
    let archive_root = String::from("archive/archive.") + &START_DATE;
    while let Ok((story, links)) = r.recv() {
        debug!("{:?} -> {:?}", story, links);
        let uri = story.get_uri();
        if let Err(e) = story.write(&archive_root) {
            error!("did not write {uri}: {e}");
        };
        let str_links = links
            .iter()
            .map(|(link, priority)| (link.as_str().to_string(), priority))
            .filter(|(link, _priority)| !known_urls.contains(link))
            .collect::<Vec<_>>();
        debug!("{} => {:?}", uri, str_links);
        for (link, priority) in str_links {
            known_urls.insert(&link);
            q.push(link, *priority);
        }
    }
    info!("Receiver completed.");
}

#[post("/", format="plain", data="<uri>")]
fn add(uri: rocket::Data, s: rocket::State<Sender<(Story, HashSet<(Url, bool)>)>>) {
    let mut stream = uri.open();

    let mut buf = String::new();
    stream.read_to_string(&mut buf).unwrap();
    info!("{buf}");

    let story = Story::new();
    let mut links = HashSet::new();
    links.insert((Url::from_str(&buf).unwrap(), true));
    s.send((story, links));
}

#[get("/")]
fn ping(s: rocket::State<Sender<(Story, HashSet<(Url, bool)>)>>) -> &'static str {
    info!("ping");
    let story = Story::new();
    let mut links = HashSet::new();
    links.insert((Url::from_str("huh://nonsense").unwrap(), true));
    s.send((story, links));
    "pong"
}

fn main() {
    env_logger::init();

    let q: PriorityInjector<String> = PriorityInjector::new();
    let mut known_urls = KnownURLHashSet::default();

    let f = std::fs::File::open("config.yaml").unwrap();
    let config: CrawlConfig = serde_yaml::from_reader(f).unwrap();

    info!("{:#?}", config);

    config.seeds.iter().for_each(|u| {
        known_urls.insert(u);
        q.push(u.to_string(), true);
    });

    let config = Arc::new(config);

    let (s, r) = crossbeam::channel::unbounded();
    scope(|scope| {
        for t in 0..config.threads {
            scope
                .builder()
                .name(format!("thread {t}"))
                .spawn(|_| {
                    run_thread(&q, s.clone(), config.clone());
                })
                .unwrap();
        }
        scope
            .builder()
            .name("receiver".to_string())
            .spawn(|_| {
                receiver(r, &q, known_urls);
            })
            .unwrap();
        scope
            .builder()
            .name("rocket".to_string())
            .spawn(|_| {
                rocket::ignite()
                //.manage(&q)
                .manage(s.clone())
                .mount("/", rocket::routes![add, ping])
                .launch()
            }).unwrap();
    }).unwrap();
}
