# pastebin-iron

This is a CLI-first Pastebin clone built on the Iron framework for Rust.

[LIVE DEMO](http://45.62.211.238:3000/)

Being sick to the teeth with PHP and finding the various Python framworks (e.g. Django, Flask) unsatisfying, I was intrigued by the [Rocket](https://rocket.rs/) web framework for Rust.
Mainly, I'm thinking that I really want a strongly-typed language and/or compiled language for web programming.
I ran through Rocket's [tutorial app](https://rocket.rs/guide/pastebin/), but found the framework lacking in many regards.
So I took the opportunity to run through the same tutorial using [Iron](http://ironframework.io/) instead.
I found this to be a far better experience.

So, to get a feel for the various frameworks that piqued my curiosity, I decided to implement this app in several different frameworks and languages:

* [Rust](https://www.rust-lang.org/) using [Iron](http://ironframework.io/) (this repository)
* [Rust](https://www.rust-lang.org/) using [Nickel](http://nickel.rs/) (TODO)
* [Julia](http://julialang.org/) using whatever framework(s)
    [may](http://juliawebstack.org/) [look](http://escher-jl.org/) [interesting](https://github.com/essenciary/Genie.jl) when I get there (TODO)
* ???

This application makes use of a number of aspects which aren't strictly necessary for a project this size
    (e.g. employs templates, staticfiles, form processing, etc.)
    because I want to get a feel for how building a real application feels.

It turns out that most of these frameworks are pretty immature, so I may end up having to implement various middleware (e.g. CSRF protection) myself.
This will be interesting!
