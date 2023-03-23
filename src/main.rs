#[macro_use]
extern crate rocket;

use chrono::{Datelike, Utc};
use crossbeam::channel::{Receiver, Sender};
use crossbeam::deque::{
    Injector, Steal,
    Steal::{Empty, Retry, Success},
};
use mt_crawl::Story;
use reqwest::blocking::Client;
use reqwest::Url;
use rocket::tokio::time::{sleep, Duration};
use rocket::State;
use rocket_prometheus::PrometheusMetrics;
use serde_derive::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::str::FromStr;
use std::sync::Arc;
use std::thread;

use rocket::data::ToByteUnit;
use rocket::tokio::io::AsyncReadExt;

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
            Success(stop) if stop == "STOP" => {
                break;
            }
            Success(uri) => {
                debug!(
                    "{}: received {}",
                    thread::current().name().unwrap_or("None"),
                    uri
                );
                match client.get(&uri).send() {
                    Ok(resp) => match lit.process(&uri, &config, resp) {
                        Ok(data) => {
                            info!("{uri}");
                            s.send(data).unwrap()
                        }
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

async fn receiver(
    r: Receiver<(Story, HashSet<(Url, bool)>)>,
    q: &PriorityInjector<String>,
    mut known_urls: impl KnownURL,
) -> std::io::Result<()> {
    let archive_root = String::from("archive/archive.") + &START_DATE;
    loop {
        if r.is_empty() {
            sleep(Duration::from_millis(100)).await;
            continue;
        }
        if let Ok((story, links)) = r.recv_timeout(Duration::from_millis(100)) {
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
    }

    info!("Receiver completed.");
    Ok(())
}

type StoryState = State<Sender<(Story, HashSet<(Url, bool)>)>>;

#[post("/", data = "<uri>")]
async fn add(uri: rocket::Data<'_>, s: &StoryState) -> &'static str {
    let mut stream = uri.open(1.kilobytes());

    let mut buf = String::new();
    stream.read_to_string(&mut buf).await.unwrap();
    info!("{buf}");

    let story = Story::new();
    let mut links = HashSet::new();
    links.insert((Url::from_str(&buf).unwrap(), true));
    s.send((story, links)).unwrap();
    "Ok"
}

#[get("/")]
async fn ping(s: &StoryState) -> &'static str {
    info!("ping");
    let story = Story::new();
    let mut links = HashSet::new();
    links.insert((Url::from_str("huh://nonsense").unwrap(), true));
    s.send((story, links)).unwrap();
    "pong"
}

#[rocket::main]
async fn main() {
    env_logger::init();

    let q = Arc::new(PriorityInjector::<String>::new());
    let mut known_urls = KnownURLHashSet::default();

    let f = std::fs::File::open("config.yaml").unwrap();
    let config: CrawlConfig = serde_yaml::from_reader(f).unwrap();

    info!("{:#?}", config);

    config.seeds.iter().for_each(|u| {
        known_urls.insert(u);
        q.push(u.to_string(), true);
    });

    let config = Arc::new(config);

    let (s, r) = crossbeam::channel::unbounded::<(Story, HashSet<(Url, bool)>)>();
    let threads = (0..config.threads)
        .map(|i| {
            let q = q.clone();
            let s = s.clone();
            let config = config.clone();
            thread::Builder::new()
                .name(format!("thread{i}"))
                .spawn(move || {
                    run_thread(&q, s.clone(), config.clone());
                })
                .unwrap()
        })
        .collect::<Vec<_>>();

    let receiver = receiver(r, &q, known_urls);
    let prometheus = PrometheusMetrics::new();
    let server = rocket::build()
        .manage(s.clone())
        .attach(prometheus.clone())
        .mount("/", rocket::routes![add, ping])
        .mount("/metrics", prometheus)
        .launch();

    futures::join!(receiver, server);
    for thread in threads {
        thread.join().unwrap();
    }
}
