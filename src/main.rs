/*
TODO:
- Limit the upload to a maximum size. If the upload exceeds that size, return a 206 partial status code. Otherwise, return a 201 created status code.
- Use the testing module to write unit tests for your pastebin.
- Dispatch a thread before launching Iron in main that periodically cleans up idling old pastes in upload/.
- Replace calls to unwrap etc. references with actual error handling

DONE:
- Ensure generated PasteID is unique.
- Set the Content-Type of the return value in upload and retrieve to text/plain.
- Support deletion of pastes by adding a new DELETE /<id> route.
- Require that the key is present and matches when doing deletion.
- Add a PUT /<id> route that allows a user with the <id> to replace the existing paste, if any.
- Generate unique key for each paste, restrict PUT and DELETE to knowing this key
- Add a web form to the index where users can manually input new pastes. Accept the form at POST /. (need to use different content-type to differentiate)
- Add a new route, GET /<id>/<lang> that syntax highlights the paste with ID <id> for language <lang>. If <lang> is not a known language, do no highlighting. Possibly validate <lang> with FromParam.
*/

#[macro_use] extern crate iron;
extern crate router;
extern crate persistent;
extern crate params;
extern crate bodyparser;

extern crate crypto;
extern crate rand;
extern crate syntect;

use std::fs;
use std::fs::File;
use std::path::Path;
use std::io::Write;
use std::io::Read;

use iron::headers::{ContentType, UserAgent};
use iron::modifiers::Header;
use iron::prelude::*;
use iron::status;
use iron::typemap::Key;

use params::{Params, Value};
use router::Router;

use crypto::hmac::Hmac;
use crypto::mac::Mac;
use crypto::sha2::Sha256;

use rand::Rng;

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet, Style};
use syntect::html::highlighted_snippet_for_string;
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;

const SOCKET: &'static str = "localhost:3000";
const BASE62: &'static [u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const HMAC_KEY: &'static [u8] = b"this is my hmac key lol :) 3456789*&^%$#W";
const ID_LEN: usize = 5;
const KEY_BYTES: usize = 8;

struct HighlighterData {
    ss: SyntaxSet,
    theme: Theme
}
impl Key for HighlighterData { type Value = HighlighterData; }
// TODO: why do we need these? how do we make them sefe? it's read only after all
unsafe impl Send for HighlighterData {}
unsafe impl Sync for HighlighterData {}


fn main() {
    let mut router = Router::new();
    router.get("/", usage, "index");
    router.get("/:paste_id", retrieve, "retrieve");
    router.get("/:paste_id/:lang", retrieve, "retrieve_lang");
    router.delete("/:paste_id/:key", delete, "delete");
    router.put("/:paste_id/:key", replace, "replace");
    router.post("/", submit, "submit");

    let mut chain = Chain::new(router);

    let ss = SyntaxSet::load_defaults_nonewlines();
    let ts = ThemeSet::load_defaults();
    let ref theme = ts.themes["base16-eighties.dark"];
    let highlighter_data = HighlighterData {ss: ss, theme: theme.clone()};
    chain.link(persistent::Read::<HighlighterData>::both(highlighter_data));

    let server = Iron::new(chain).http(SOCKET).unwrap();
    println!("listening on http://{} ({})", SOCKET, server.socket);
}


// Note: webform is multipart/form-data so that raw post data yields None. Doing
// so allows us to unambiguously differentiate between a "data" variable (from
// the web form) and a raw post that happens contain urlencoded query params.
// TODO: determine if it is poor style to have multipart forms without file upload?
fn usage(_: &mut Request) -> IronResult<Response> {
    Ok(Response::with((status::Ok, Header(ContentType::html()), format!("<html><head></head><body><pre>
    USAGE

      POST /
          accepts raw data in the body of the request and responds with a URL of
          a page containing the body's content:
          eg: echo \"hello world\" | curl --data-binary @- http://{socket}

      GET /&lt;id&gt;
          retrieves the content for the paste with id `&lt;id&gt;`.
          eg: curl http://{socket}/{id}

      GET /&lt;id&gt;/&lt;ext&gt;
          retrieves the contents of the paste with id `id`, with syntax highlighting
          associated with the file extension `ext`.
          eg: curl http://{socket}/{id}/{ext}

      DELETE /&lt;id&gt;/&lt;key&gt;
          deletes the paste with id `&lt;id&gt;`.
          eg: curl -X DELETE http://{socket}/{id}/{key}

      PUT /&lt;id&gt;/&lt;key&gt;
          replaces the contents of the paste with id `&lt;id&gt;`.
          eg: echo \"hello world\" | curl -X PUT --data-binary @- http://{socket}/{id}/{key}</pre>
    <hr>
    or use this form:
    <form method=\"post\" enctype=\"multipart/form-data\">
     <textarea name=\"data\" style=\"display: block; width: 500px; height: 300px\"></textarea>
     <input type=\"submit\">
    </form>
    </body></html>\n", socket = SOCKET, id = "MySrc", key = "a7772362cf6e2c36", ext = "rs"))))
}


// TODO: determine whether bodyparser can replace Params ("parses body into a struct using Serde")
fn submit(req: &mut Request) -> IronResult<Response> {
    // get paste contents, either raw post or data param
    let raw_body = itry!(req.get::<bodyparser::Raw>());
    let paste = match raw_body {
        Some(paste) => paste,
        None => {
            // TODO: determine why this needs .get_ref, when we used .get above for raw post
            let params = req.get_ref::<Params>().unwrap();
            match params.find(&[&"data"]) {
                Some(&Value::String(ref data)) => data.clone().to_string(),
                _ => panic!("no paste data")
            }
        }
    };
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
    let url = format!("http://{socket}/{id}", socket = SOCKET, id = id);

    let mut f = itry!(File::create(path));
    itry!(f.write_all(paste.as_bytes()));
    Ok(Response::with((status::Ok, format!("View URL: {url}\nEdit URL: {url}/{key}\n", url = url, key = gen_key(id)))))
}

fn retrieve(req: &mut Request) -> IronResult<Response> {
    // TODO: borrow checker wants this above params, but that's not ideal...
    let arc = req.get::<persistent::Read<HighlighterData>>().expect("getting arc for highlighting");
    let highlighter_data = arc.as_ref();
    // ok now that's out of the way, lets get params
    let params = req.extensions.get::<Router>().unwrap();
    // TODO: "ref" appears unnecessary -- determine why it's here
    let ref id = params.find("paste_id").unwrap_or("");
    let lang = params.find("lang");

    let mut f = itry!(File::open(format!("uploads/{id}", id = id)));
    let mut buffer = String::new();
    itry!(f.read_to_string(&mut buffer));

    match lang {
        Some(lang) => {
            // syntax highlighting
            let syntax = highlighter_data.ss.find_syntax_by_extension(lang).unwrap_or_else(|| highlighter_data.ss.find_syntax_plain_text());
            let mut output = String::new();
            let show_html_output = match req.headers.get::<UserAgent>() {
                // TODO: are these calls to to_string() necessary?
                Some(&UserAgent(ref string)) => string[..5].to_string() != "curl/".to_string(),
                _ => true
            };
            if show_html_output {
                output = highlighted_snippet_for_string(&buffer, syntax, &highlighter_data.theme);
                Ok(Response::with((status::Ok, Header(ContentType::html()), format!("{}{}{}",
                    "<html><head><style>body {margin: 0} body > pre { padding: 10px } pre {margin: 0; padding: 0px}</style></head><body>",
                    output,
                    "</body></html>\n"))))
            } else {
                let mut highlighter = HighlightLines::new(syntax, &highlighter_data.theme);
                for line in buffer.lines() {
                    let ranges: Vec<(Style, &str)> = highlighter.highlight(line);
                    let escaped;
                    escaped = as_24_bit_terminal_escaped(&ranges[..], false);
                    output += &format!("{}\n", escaped);
                }
                Ok(Response::with((status::Ok, output)))
            }
        },
        // no syntax highlighting
        None => {
            Ok(Response::with((status::Ok, buffer)))
        }
    }
}

fn delete(req: &mut Request) -> IronResult<Response> {
    let params = req.extensions.get::<Router>().unwrap();
    let ref id = params.find("paste_id").unwrap_or("/");
    let ref key = params.find("key").unwrap_or("/");
    // verify file
    let path = format!("uploads/{id}", id = id);
    if !Path::new(&path).exists() {
        return Ok(Response::with((status::NotFound, format!("Paste {} does not exist.\n", id))));
    }
    // verify key
    if *key != gen_key(id.to_string()) {
        return Ok(Response::with((status::Unauthorized, "Invalid key.\n")));
    }
    itry!(fs::remove_file(path));
    Ok(Response::with((status::Ok, format!("Paste {} deleted.\n", id))))
}

fn replace(req: &mut Request) -> IronResult<Response> {
    // body parsing happens first because it does an immutable borrow
    // TODO: determine how to now require this.
    let body = itry!(req.get::<bodyparser::Raw>()).unwrap();

    let params = req.extensions.get::<Router>().unwrap();
    let ref id = params.find("paste_id").unwrap_or("/");
    let ref key = params.find("key").unwrap_or("/");
    // verify file
    let path = format!("uploads/{id}", id = id);
    if !Path::new(&path).exists() {
        return Ok(Response::with((status::NotFound, format!("Paste {} does not exist.\n", id))));
    }
    // verify key
    if *key != gen_key(id.to_string()) {
        return Ok(Response::with((status::Unauthorized, "Invalid key.\n")));
    }

    let mut f = itry!(File::create(path));
    itry!(f.write_all(body.as_bytes()));
    Ok(Response::with((status::Ok, format!("http://{socket}/{id} overwritten.\n", socket=SOCKET, id = id))))
}

fn generate_id(size: usize) -> String {
    let mut id = String::with_capacity(size);
    let mut rng = rand::thread_rng();
    for _ in 0..size {
        id.push(BASE62[rng.gen::<usize>() % 62] as char);
    }
    id
}

fn gen_key(input: String) -> String {
    let mut hmac = Hmac::new(Sha256::new(), HMAC_KEY);
    hmac.input(input.as_bytes());
    let hmac_result = hmac.result();
    let key: String = hmac_result.code().iter()
        .take(KEY_BYTES)
        .map(|b| format!("{:02X}", b))
        .collect();
    key.to_lowercase()
}
