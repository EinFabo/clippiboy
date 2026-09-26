const ORIGIN = "https://einfabo.github.io/clippiboy";
const HOME = "https://clippiboy.com";

export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    // One address: www goes to the bare domain.
    if (url.hostname !== "clippiboy.com") {
      return Response.redirect(`${HOME}${url.pathname}${url.search}`, 301);
    }
    if (request.method !== "GET" && request.method !== "HEAD") {
      return new Response("Method not allowed", { status: 405, headers: { Allow: "GET, HEAD" } });
    }
    const upstream = await fetch(`${ORIGIN}${url.pathname}${url.search}`, {
      method: request.method,
      redirect: "manual",
      // Short, so a release baked into the page shows within minutes.
      cf: { cacheEverything: true, cacheTtl: 300 },
    });
    const response = new Response(upstream.body, upstream);
    // GitHub's own redirects (/img → /img/) point at github.io; keep them here.
    const location = response.headers.get("Location");
    if (location?.startsWith(ORIGIN)) {
      response.headers.set("Location", HOME + location.slice(ORIGIN.length));
    } else if (location?.startsWith("/clippiboy")) {
      response.headers.set("Location", location.slice("/clippiboy".length) || "/");
    }
    return response;
  },
};
