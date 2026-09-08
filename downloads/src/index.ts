// NiumaTerm downloads: a caching front for this repository's release assets.
//
// Clients on networks with a poor path to GitHub pay that cost on every update,
// and the archive is the large part of one. Routing it through Cloudflare turns
// the slow leg into a backbone hop the edge makes once per cached object, while
// GitHub Releases stays the only place a build is published.

// Every asset this serves lives under one release download prefix, and an
// accepted path maps onto it one for one. Building the upstream URL from a
// fixed prefix plus two validated segments is what keeps the route from
// becoming an open proxy: nothing a caller sends can move the fetch off this
// repository.
const RELEASE_PREFIX = "https://github.com/f32y/NiumaTerm/releases/download/";

// A tag carries a version or the commit a nightly was built from, and an asset
// name carries the tag, so both stay inside this set. Rejecting anything else
// keeps traversal and query smuggling out of the upstream URL.
const SEGMENT = /^[A-Za-z0-9._-]+$/;

// A tag names one source revision, so the bytes behind a path hold still in
// practice. The window bounds how long a republished asset stays hidden behind
// a cached copy, which is what the nightly workflow does when it rebuilds a
// revision under the tag that revision already has.
const CACHE_TTL_SECONDS = 86400;

export default {
  async fetch(request: Request): Promise<Response> {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("expected GET or HEAD\n", {
        status: 405,
        headers: { Allow: "GET, HEAD" },
      });
    }

    const segments = new URL(request.url).pathname.split("/").slice(1);
    if (segments.length !== 2 || !segments.every((s) => SEGMENT.test(s))) {
      return new Response("not found\n", { status: 404 });
    }
    const [tag, asset] = segments;

    // Range is forwarded so a client resuming a partial download still can; the
    // edge answers it from the whole cached object once it holds one.
    const headers = new Headers();
    const range = request.headers.get("Range");
    if (range) {
      headers.set("Range", range);
    }

    const upstream = await fetch(`${RELEASE_PREFIX}${tag}/${asset}`, {
      method: request.method,
      headers,
      cf: { cacheEverything: true, cacheTtl: CACHE_TTL_SECONDS },
    });

    // A fetched response carries immutable headers, so the cache directive this
    // hands to clients goes on a copy of it.
    const response = new Response(upstream.body, upstream);
    response.headers.set("Cache-Control", `public, max-age=${CACHE_TTL_SECONDS}`);

    return response;
  },
};
