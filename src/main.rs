#[macro_use] extern crate iron;
extern crate router;
extern crate params;
extern crate bodyparser;
extern crate handlebars_iron;
extern crate staticfile;
extern crate mount;

extern crate chrono;
extern crate crypto;
#[macro_use] extern crate lazy_static;
extern crate rand;
extern crate syntect;

use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::io::Write;
use std::io::Read;
use std::thread;
use std::time;

use iron::headers::{ContentType, UserAgent, Host};
use iron::middleware::BeforeMiddleware;
use iron::modifiers::Header;
use iron::prelude::*;
use iron::status;

use handlebars_iron::{HandlebarsEngine, DirectorySource, Template};
use mount::Mount;
use params::{Params, Value};
use router::Router;
use staticfile::Static;

use chrono::{DateTime, UTC};

use crypto::hmac::Hmac;
use crypto::mac::Mac;
use crypto::sha2::Sha256;

use rand::Rng;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet, Style};
use syntect::html::highlighted_snippet_for_string;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

const SOCKET: &'static str = "127.0.0.1:3000";
const BASE62: &'static [u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_LEN: usize = 5;
const KEY_BYTES: usize = 8;
const MAX_PASTE_BYTES: usize = 2 * 1024 * 1024; // 2 MB

lazy_static! {
    static ref HMAC_KEY: String = {
        let mut file = match File::open("hmac_key.txt") {
            Ok(f) => f,
            Err(_) => return String::new()
        };
        let mut key = String::new();
        file.read_to_string(&mut key).expect("reading HMAC key file");
        key
    };

    static ref HL_THEME: Theme = {
        let ts = ThemeSet::load_defaults();
        let theme = &ts.themes["base16-eighties.dark"];
        theme.clone()
    };
}

// SyntaxSet does not implement Copy/Sync, so we do it like this.
// see https://github.com/trishume/syntect/issues/20
thread_local! {
    static SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_nonewlines();
}

#[derive(Debug)]
enum HighlightedText {
    Terminal(String),
    Html(String),
    Error(String)
}



struct LoggingMiddleware;
impl BeforeMiddleware for LoggingMiddleware {
    fn before(&self, req: &mut Request) -> IronResult<()> {
        let utc: DateTime<UTC> = UTC::now();
        println!("[{}] [{}]: {}", req.remote_addr, utc.format("%Y-%m-%d %H:%M:%S"), req.url);
        Ok(())
    }
}


fn main() {
    if HMAC_KEY.as_bytes().len() == 0 {
        println!("You must set a key in hmac_key.txt");
        std::process::exit(1);
    }

    let mut router = Router::new();
    router.get("/", usage, "index");
    router.get("/help", help, "help");
    router.get("/:paste_id", retrieve, "retrieve");
    router.get("/:paste_id/:lang", retrieve, "retrieve_lang");
    router.delete("/:paste_id", delete, "delete_nokey");
    router.delete("/:paste_id/:key", delete, "delete");
    router.put("/:paste_id/:key", replace, "replace");
    router.post("/", submit, "submit");

    let mut mount = Mount::new();
    mount.mount("/", router)
         .mount("/webupload", Static::new(Path::new("./static/webupload.html")));

    let mut hbse = HandlebarsEngine::new();
    hbse.add(Box::new(DirectorySource::new("./templates/", ".hbs")));

    // load templates from all registered sources
    if let Err(r) = hbse.reload() {
        panic!("{}", r.cause);
    }

    let mut chain = Chain::new(mount);
    chain.link_before(LoggingMiddleware);
    chain.link_after(hbse);
    let server = Iron::new(chain).http(SOCKET).unwrap();

    println!("Listening on http://{} ({})", SOCKET, server.socket);

    // every day, delete pastes > 30 days old
    thread::spawn(move || {
        let one_day = time::Duration::from_secs(60*60*24);
        let thirty_days = one_day * 30;
        println!("Pastes are deleted when they are 30 days old.");
        loop {
            let now = time::SystemTime::now();
            let files = fs::read_dir("./uploads").unwrap();
            for file in files {
                let path = file.unwrap().path();
                let attr = fs::metadata(&path).unwrap();
                let last_modified = attr.modified().expect("reading last modified time");
                if now.duration_since(last_modified).unwrap() > thirty_days {
                    fs::remove_file(path).expect("deleting file");
                }
            }
            thread::sleep(one_day);
        }
    });
}


fn usage(req: &mut Request) -> IronResult<Response> {
    let mut resp = Response::new();
    resp.set_mut(Header(ContentType::plaintext()));

    let mut data = BTreeMap::new();
    data.insert("host".to_string(), get_hostname(req));
    data.insert("id".to_string(), "vxcRz".to_string());
    data.insert("key".to_string(), "a7772362cf6e2c36".to_string());
    data.insert("ext".to_string(), "rs".to_string());

    resp.set_mut(Template::new("index", data)).set_mut(status::Ok);
    Ok(resp)
}

fn help(req: &mut Request) -> IronResult<Response> {
    // TODO: combine this with usage above.
    let mut resp = Response::new();
    resp.set_mut(Header(ContentType::plaintext()));

    let mut data = BTreeMap::new();
    data.insert("host".to_string(), get_hostname(req));
    data.insert("id".to_string(), "vxcRz".to_string());
    data.insert("key".to_string(), "a7772362cf6e2c36".to_string());
    data.insert("ext".to_string(), "rs".to_string());

    resp.set_mut(Template::new("help", data)).set_mut(status::Ok);
    Ok(resp)
}

// Note: webform is multipart/form-data so that raw post data yields None.
// Doing so allows us to unambiguously differentiate between a "data"
// variable (from the web form) and a raw post that happens contain
// urlencoded query params. Is this poor style?
// TODO: determine whether bodyparser can replace Params ("parses body into a struct using Serde")
fn submit(req: &mut Request) -> IronResult<Response> {
    // get paste contents, either raw post or data param
    let raw_body = match req.get::<bodyparser::Raw>() {
        Ok(body) => body,
        Err(e) => return Ok(Response::with((status::BadRequest, format!("Invalid paste data submitted: {}.\n", e.detail))))
    };
    let paste = match raw_body {
        Some(paste) => paste,
        None => {
            // TODO: determine why this needs .get_ref, when we used .get above for raw post
            let params = req.get_ref::<Params>().unwrap();
            match params.find(&["data"]) {
                Some(&Value::String(ref data)) => data.clone().to_string(),
                _ => return Ok(Response::with((status::BadRequest, "No paste data submitted.\n")))
            }
        }
    };
    // verify max size before saving it
    if paste.len() > MAX_PASTE_BYTES {
        return Ok(Response::with((status::BadRequest, format!("Pastes may not be more than {} MB.\n", MAX_PASTE_BYTES/1048576))))
    }
    // get paste ID and URL
    let mut id: String;
    let mut path: String;
    let mut double_id_len = ID_LEN * 2; // so we increase by 1 every two loops
    loop {
        id = generate_id(double_id_len / 2);
        path = format!("uploads/{id}", id = id);
        if !Path::new(&path).exists() {
            break;
        }
        double_id_len += 1;
    }
    let url = format!("https://{host}/{id}", host = get_hostname(req), id = id);

    let mut f = itry!(File::create(path));
    itry!(f.write_all(paste.as_bytes()));
    Ok(Response::with((status::Created, format!("View URL: {url}\nEdit URL: {url}/{key}\n", url = url, key = gen_key(&id)))))
}

fn retrieve(req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    // TODO: "ref" appears unnecessary -- determine why it's here
    let id = &params.find("paste_id").unwrap_or("");
    let lang = params.find("lang");

    let mut f = match File::open(format!("uploads/{id}", id = id)) {
        Ok(f) => f,
        Err(_) => return Ok(Response::with((status::NotFound, format!("Paste {} does not exist\n", id))))
    };

    let mut buffer = String::new();
    itry!(f.read_to_string(&mut buffer));

    match lang {
        Some(lang) => {
            // syntax highlighting
            let html_output = !is_curl(req);
            match highlight(buffer, lang, html_output) {
                HighlightedText::Terminal(s) => Ok(Response::with((status::Ok, s))),
                HighlightedText::Html(s) => {
                    let mut resp = Response::new();
                    let mut data = BTreeMap::new();
                    data.insert("paste".to_string(), s);
                    resp.set_mut(Template::new("paste_html", data)).set_mut(status::Ok);
                    Ok(resp)
                },
                HighlightedText::Error(s) => Ok(Response::with((status::BadRequest, format!("Invalid request: {}.\n", s))))
            }
        },
        // no syntax highlighting
        None => {
            Ok(Response::with((status::Ok, buffer)))
        }
    }
}

fn delete(req: &mut Request) -> IronResult<Response> {
    let (id, path) = match validate_key_id(req) {
        Ok((id, path)) => (id, path),
        Err(reason) => return Ok(Response::with((status::BadRequest, format!("Invalid request: {}.\n", reason))))
    };
    // delete file
    itry!(fs::remove_file(path));
    Ok(Response::with((status::Ok, format!("Paste {} deleted.\n", id))))
}

fn replace(req: &mut Request) -> IronResult<Response> {
    let (id, path) = match validate_key_id(req) {
        Ok((id, path)) => (id, path),
        Err(reason) => return Ok(Response::with((status::BadRequest, format!("Invalid request: {}.\n", reason))))
    };
    // write body
    let paste = itry!(req.get::<bodyparser::Raw>()).unwrap();
    // verify max size before saving it
    if paste.len() > MAX_PASTE_BYTES {
        return Ok(Response::with((status::BadRequest, format!("Pastes may not be more than {} MB.\n", MAX_PASTE_BYTES/1048576))))
    }
    let mut f = itry!(File::create(path));
    itry!(f.write_all(paste.as_bytes()));
    Ok(Response::with((status::Ok, format!("https://{host}/{id} overwritten.\n", host=get_hostname(req), id = id))))
}





fn validate_key_id(req: &Request) -> Result<(String, String), String> {
    let params = req.extensions.get::<Router>().unwrap();
    let id = params.find("paste_id").unwrap_or("").to_string();
    let path = format!("uploads/{id}", id = id);
    if !Path::new(&path).exists() {
        return Err(format!("Paste {} does not exist", id));
    }
    let key = params.find("key").unwrap_or("");
    if key != gen_key(&id) {
        return Err("Key is not valid".to_string());
    }
    Ok((id, path))
}

fn generate_id(size: usize) -> String {
    let mut id = String::with_capacity(size);
    let mut rng = rand::thread_rng();
    for _ in 0..size {
        id.push(BASE62[rng.gen::<usize>() % 62] as char);
    }
    id
}

fn gen_key(input: &str) -> String {
    let mut hmac = Hmac::new(Sha256::new(), HMAC_KEY.as_bytes());
    hmac.input(input.as_bytes());
    let hmac_result = hmac.result();
    let key: String = hmac_result.code().iter()
        .take(KEY_BYTES)
        .map(|b| format!("{:02X}", b))
        .collect();
    key.to_lowercase()
}

fn get_hostname(req: &Request) -> String {
    match req.headers.get::<Host>() {
        Some(h) => {
            let mut host = h.hostname.to_owned();
            if let Some(port) = h.port {
                host = host + ":" + &port.to_string();
            }
            return host
        },
        _ => SOCKET.to_owned()
    }
}

fn is_curl(req: &Request) -> bool {
    match req.headers.get::<UserAgent>() {
        Some(&UserAgent(ref string)) => string.starts_with("curl/"),
        _ => true
    }
}

fn highlight(buffer: String, lang: &str, html: bool) -> HighlightedText {
    SYNTAX_SET.with(|ss| {
        let syntax = ss.find_syntax_by_extension(lang).unwrap_or_else(|| ss.find_syntax_plain_text());
        if syntax.name == "Plain Text" {
            return HighlightedText::Error(format!("Requested highlight \"{}\" not available", lang));
        }
        if html {
            HighlightedText::Html(highlighted_snippet_for_string(&buffer, syntax, &HL_THEME))
        } else {
            let mut highlighter = HighlightLines::new(syntax, &HL_THEME);
            let mut output = String::new();
            for line in buffer.lines() {
                let ranges: Vec<(Style, &str)> = highlighter.highlight(line);
                let escaped;
                escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                output += &format!("{}\n", escaped);
            }
            HighlightedText::Terminal(output)
        }
    })
}
