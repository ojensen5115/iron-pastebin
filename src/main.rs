/*
TODO:
- Limit the upload to a maximum size. If the upload exceeds that size, return a 206 partial status code. Otherwise, return a 201 created status code.
- Add a new route, GET /<id>/<lang> that syntax highlights the paste with ID <id> for language <lang>. If <lang> is not a known language, do no highlighting. Possibly validate <lang> with FromParam.
- Use the testing module to write unit tests for your pastebin.
- Dispatch a thread before launching Iron in main that periodically cleans up idling old pastes in upload/.

DONE:
- Ensure generated PasteID is unique.
- Set the Content-Type of the return value in upload and retrieve to text/plain.
- Support deletion of pastes by adding a new DELETE /<id> route.
- Require that the key is present and matches when doing deletion.
- Add a PUT /<id> route that allows a user with the <id> to replace the existing paste, if any.
- Generate unique key for each paste, restrict PUT and DELETE to knowing this key
- Add a web form to the index where users can manually input new pastes. Accept the form at POST /. (need to use different content-type to differentiate)
*/

#[macro_use] extern crate iron;
extern crate router;
extern crate params;
extern crate bodyparser;
extern crate crypto;

extern crate rand;
use rand::Rng;

use std::fs;
use std::fs::File;
use std::path::Path;
use std::io::Write;
use std::io::Read;

use crypto::hmac::Hmac;
use crypto::mac::Mac;
use crypto::sha2::Sha256;

use iron::headers::ContentType;
use iron::modifiers::Header;
use iron::prelude::*;
use iron::status;

use params::{Params, Value};
use router::Router;

const SOCKET: &'static str = "localhost:3000";
const BASE62: &'static [u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const HMAC_KEY: &'static [u8] = b"this is my hmac key lol :) 3456789*&^%$#W";
const ID_LEN: usize = 5;
const KEY_BYTES: usize = 8;

fn main() {
    let mut router = Router::new();
    router.get("/", usage, "index");
    router.get("/:paste_id", retrieve, "retrieve");
    router.get("/:paste_id/:key", invalid_method, "invalid_method");
    router.delete("/:paste_id/:key", delete, "delete");
    router.put("/:paste_id/:key", replace, "replace");
    router.post("/", submit, "submit");

    let server = Iron::new(router).http(SOCKET).unwrap();
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
          retrieves the content for the paste with id `&lt;id&gt;`
          eg: curl http://{socket}/{id}

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
    </body></html>", socket = SOCKET, id = "fZWK3", key = "a7772362cf6e2c36"))))
}



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
    loop {
        id = generate_id(ID_LEN);
        path = format!("uploads/{id}", id = id);
        if !Path::new(&path).exists() {
            break;
        }
    }
    let url = format!("http://{socket}/{id}", socket = SOCKET, id = id);

    let mut f = itry!(File::create(path));
    itry!(f.write_all(paste.as_bytes()));
    Ok(Response::with((status::Ok, format!("View URL: {url}\nEdit URL: {url}/{key}\n", url = url, key = gen_key(id)))))
}

fn retrieve(req: &mut Request) -> IronResult<Response> {
    let ref id = req.extensions.get::<Router>()
           .unwrap().find("paste_id").unwrap_or("/");

    let mut f = itry!(File::open(format!("uploads/{id}", id = id)));
    let mut buffer = String::new();
    itry!(f.read_to_string(&mut buffer));
    Ok(Response::with((status::Ok, buffer)))
}

fn delete(req: &mut Request) -> IronResult<Response> {
    let ref id = req.extensions.get::<Router>().unwrap().find("paste_id").unwrap_or("/");
    let ref key = req.extensions.get::<Router>().unwrap().find("key").unwrap_or("/");
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
    let body = itry!(req.get::<bodyparser::Raw>()).unwrap();
    let ref id  = req.extensions.get::<Router>().unwrap().find("paste_id").unwrap_or("/");
    let ref key = req.extensions.get::<Router>().unwrap().find("key").unwrap_or("/");
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

fn invalid_method(_: &mut Request) -> IronResult<Response> {
    Ok(Response::with((status::Ok, "You issued a GET request to an edit URL.\nTry PUT or DELETE instead, or remove the key.")))
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
