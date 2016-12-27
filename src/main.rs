/*

TODO:
- Add a web form to the index where users can manually input new pastes. Accept the form at POST /. Use format and/or rank to specify which of the two POST / routes should be called.
- Support deletion of pastes by adding a new DELETE /<id> route. Use PasteID to validate <id>.
- Limit the upload to a maximum size. If the upload exceeds that size, return a 206 partial status code. Otherwise, return a 201 created status code.
- Require that the key is present and matches when doing deletion. Use one of Rocket’s core traits to do the key validation.
- Add a PUT /<id> route that allows a user with the key for <id> to replace the existing paste, if any.
- Add a new route, GET /<id>/<lang> that syntax highlights the paste with ID <id> for language <lang>. If <lang> is not a known language, do no highlighting. Possibly validate <lang> with FromParam.
- Use the testing module to write unit tests for your pastebin.
- Dispatch a thread before launching Rocket in main that periodically cleans up idling old pastes in upload/.

DONE:
- Set the Content-Type of the return value in upload and retrieve to text/plain.
- Return a unique “key” after each upload

*/

#[macro_use] extern crate iron;
extern crate router;
extern crate bodyparser;

extern crate rand;
use rand::Rng;

use std::fs::File;
use std::path::Path;
use std::io::Write;
use std::io::Read;

use iron::prelude::*;
use iron::status;

use router::Router;

fn main() {
    let mut router = Router::new();
    router.get("/", usage, "index");
    router.get("/:paste_id", retrieve, "retrieve");
    router.post("/", submit, "submit");

    println!("http://localhost:3000/");
    Iron::new(router).http("localhost:3000").unwrap();
}

fn usage(_: &mut Request) -> IronResult<Response> {
    Ok(Response::with((status::Ok, "
    USAGE

      POST /

          accepts raw data in the body of the request and responds with a URL of
          a page containing the body's content:

          echo \"hello world\" | curl --data-binary @- http://localhost:3000

      GET /<id>

          retrieves the content for the paste with id `<id>`")))
}

const BASE62: &'static [u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_LEN: usize = 5;

fn submit(req: &mut Request) -> IronResult<Response> {
    let body = itry!(req.get::<bodyparser::Raw>()).unwrap();

    let mut id: String;
    let mut path: String;
    loop {
        id = generate_id(ID_LEN);
        path = format!("uploads/{id}", id = id);
        if !Path::new(&path).exists() {
            break;
        }
    }
    let url = format!("http://localhost:3000/{id}", id = id);

    let mut f = itry!(File::create(path));
    itry!(f.write_all(body.as_bytes()));
    Ok(Response::with((status::Ok, url + "\n")))
}

fn retrieve(req: &mut Request) -> IronResult<Response> {
    let ref id = req.extensions.get::<Router>()
           .unwrap().find("paste_id").unwrap_or("/");

    let mut f = itry!(File::open(format!("uploads/{id}", id = id)));
    let mut buffer = String::new();
    itry!(f.read_to_string(&mut buffer));
    Ok(Response::with((status::Ok, buffer)))
}

fn generate_id(size: usize) -> String {
    let mut id = String::with_capacity(size);
    let mut rng = rand::thread_rng();
    for _ in 0..size {
        id.push(BASE62[rng.gen::<usize>() % 62] as char);
    }
    id
}
