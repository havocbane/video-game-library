// Humble Bundle library → CSV for Video Game Library's "CSV file" importer.
//
// How to use:
//   1. Sign in at https://www.humblebundle.com and open https://www.humblebundle.com/home/library
//   2. Open the browser console (F12 → Console). Chrome/Edge may ask you to
//      type "allow pasting" first.
//   3. Paste this whole file and press Enter. A file named
//      humble-library-YYYY-MM-DD.csv downloads when it finishes.
//
// It only reads your own library from humblebundle.com and builds the file in
// your browser; nothing is sent anywhere else.
//
// How it works: Humble's library page (2026 redesign) has no JSON API. It
// loads items through a Next.js server action called "filterUserLibrary" and
// renders bundle contents on /home/library/group/<id>. This script calls the
// same action, reads the bundle pages, and picks library items out of the
// React Server Component responses. If Humble changes the site again, the
// console output says which step failed.
//
// Rows: one per game, from store keys (Steam, GOG, Epic...) and DRM-free
// downloads. Soundtracks, ebooks, videos and expired keys are skipped. Extra
// columns (Key status, Bundle) are for you; the importer ignores them.

(() => {
  "use strict";

  const ACTION_NAME = "filterUserLibrary";
  // Seen in Humble's scripts in November 2026; looked up again at run time.
  const FALLBACK_ACTION_ID = "607622c68ed0fa8a71acd7ff343f9f3366703af38e";
  const PAGE_SIZE = 5; // what the library page itself asks for
  // Diagnostic mode: run `window.HUMBLE_EXPORT_DIAGNOSE = true` before pasting
  // the script to read only a few pages and print what download items look like.
  const DIAGNOSE = typeof window !== "undefined" && window.HUMBLE_EXPORT_DIAGNOSE === true;
  const MAX_PAGES = DIAGNOSE ? 6 : 2000;
  const KNOWN_PLATFORMS = new Set([
    "android", "asmjs", "audio", "comedy", "ebook", "image", "ios", "linux", "mac", "other", "unitywasm", "video", "windows",
  ]);
  const DOWNLOAD_PLATFORMS = { windows: "PC", mac: "PC", linux: "PC", android: "Android" };
  const STORES = {
    steam: "Steam", gog: "GOG", epic: "Epic", origin: "EA app", ea: "EA app", uplay: "Ubisoft Connect",
    ubisoft: "Ubisoft Connect", battlenet: "Battle.net", rockstar: "Rockstar", microsoft: "Microsoft Store",
  };

  // ---------- turning library items into rows ----------

  // In key types and bundle names "switch" means the console; in game titles it
  // usually doesn't ("Mighty Switch Force!"), so titles must say "Nintendo Switch".
  const nintendoSystems = (strict) => [
    [/wii[\s_-]*u/i, "Wii U"],
    [/\b(new\s*)?[23]ds\b/i, "Nintendo 3DS"],
    [strict ? /nintendo[\s_-]*switch/i : /switch/i, "Nintendo Switch"],
  ];

  /// Which Nintendo system a key is for. Humble sold Wii U and 3DS keys as well
  /// as Switch ones, and the key type may only say "nintendo", so the bundle
  /// name and then the titles are checked too. The first hint that names
  /// exactly one system wins; a bundle like "Wii U & 3DS" is no help. With no
  /// clue, it's just "Nintendo" rather than a guess.
  function nintendoPlatform(type, { bundle, titles = [] } = {}) {
    const hints = [[type, false], [bundle, false], ...titles.map((t) => [t, true])];
    for (const [hint, strict] of hints) {
      if (typeof hint !== "string") continue;
      const named = nintendoSystems(strict).filter(([re]) => re.test(hint));
      if (named.length === 1) return named[0][1];
    }
    return "Nintendo";
  }

  /// Store and platform for a key type. `hints` ({ bundle, titles }) narrow
  /// down Nintendo keys.
  function keyStore(type, hints) {
    const t = typeof type === "string" ? type.toLowerCase() : "";
    if (/switch|nintendo|wii|3ds/.test(t)) return { store: "Nintendo", platform: nintendoPlatform(t, hints) };
    if (t.includes("xbox")) return { store: "Xbox", platform: "Xbox" };
    if (t.includes("playstation") || t.includes("psn")) return { store: "PlayStation", platform: "PlayStation" };
    return { store: STORES[t] || (t ? t[0].toUpperCase() + t.slice(1) : "Humble"), platform: "PC" };
  }

  const isStoreType = (type) => {
    const t = type.toLowerCase();
    return t in STORES || /switch|nintendo|wii|3ds|xbox|playstation|psn/.test(t);
  };

  /// Drops store suffixes Humble sometimes puts in key names ("Celeste Steam Key").
  function cleanTitle(name) {
    return (typeof name === "string" ? name : "").replace(/\s*[-–(]?\s*(steam|gog|epic|origin|uplay)\s+key\)?\s*$/i, "").trim();
  }

  /// Library items (as returned by Humble) → CSV rows. `bundleOf` maps item ids
  /// to the title of the bundle group they came from. If `breakdown` (a Map) is
  /// given, every item is counted under what became of it, with a few example
  /// titles each. Exported for testing.
  function itemsToRows(items, bundleOf = new Map(), breakdown = null) {
    const rows = new Map(); // "title|store" → row
    const kinds = new Set(); // what the current item contributed: "store key", "DRM-free"...
    const add = (row, kind) => {
      if (!row.title) return;
      kinds.add(kind);
      const key = `${row.title.toLowerCase()}|${row.store}`;
      if (!rows.has(key)) rows.set(key, row);
    };
    const note = (category, item, detail = "") => {
      if (!breakdown) return;
      const entry = breakdown.get(category) ?? { count: 0, examples: [] };
      entry.count++;
      const title = typeof item.title === "string" ? item.title : String(item.id);
      if (entry.examples.length < 5) entry.examples.push(detail ? `${title} (${detail})` : title);
      breakdown.set(category, entry);
    };

    // Humble's data is loosely shaped; treat anything that isn't a list as empty.
    const list = (v) => (Array.isArray(v) ? v.filter((x) => x && typeof x === "object") : []);

    let skipped = 0;
    for (const item of items) {
      try {
        rowsFor(item);
      } catch (err) {
        note("skipped: couldn't read", item);
        if (!skipped++) console.warn("Humble export: skipped an entry it couldn't read:", err, item);
      }
    }
    if (skipped) console.warn(`Humble export: ${skipped} entries couldn't be read and were left out.`);
    return [...rows.values()].sort((a, b) => a.title.localeCompare(b.title));

    function rowsFor(item) {
      if (item.itemType === "entitlement-group") {
        note("skipped: bundle group (its games are listed separately)", item);
        return;
      }
      const bundle = bundleOf.get(item.id) || "";
      const keys = list(item.entitlements?.keys);
      const downloads = list(item.entitlements?.downloads);
      const deliveryMethods = Array.isArray(item.deliveryMethods) ? item.deliveryMethods : [];
      const otherPlatforms = new Set(); // download platforms that aren't games
      kinds.clear();

      for (const key of keys) {
        if (key.expired) continue;
        const { store, platform } = keyStore(key.type || deliveryMethods[0], { bundle, titles: [key.title, item.title] });
        add({
          title: cleanTitle(key.title || item.title),
          platform,
          store,
          keyStatus: key.redeemedAt ? "revealed" : "not revealed",
          bundle,
        }, "store key");
      }

      for (const dl of downloads) {
        const raw = list(dl.downloads).map((d) => String(d.platform || "").toLowerCase());
        const platforms = raw.map((p) => DOWNLOAD_PLATFORMS[p]);
        if (!platforms.some(Boolean)) {
          raw.filter(Boolean).forEach((p) => otherPlatforms.add(p)); // soundtrack, ebook, video...
          continue;
        }
        add({
          title: cleanTitle(dl.title || item.title),
          platform: platforms.includes("PC") ? "PC" : "Android",
          store: "Humble",
          keyStatus: "DRM-free",
          bundle,
        }, "DRM-free");
      }

      // Items whose entitlements said nothing usable, but whose delivery method
      // is a store key (Steam, Epic, Switch...). Other methods (downloads,
      // ebooks) don't make a game on their own.
      const method = deliveryMethods.find((m) => typeof m === "string" && isStoreType(m));
      if (!kinds.size && method) {
        const { store, platform } = keyStore(method, { bundle, titles: [item.title] });
        add({ title: cleanTitle(item.title), platform, store, keyStatus: "", bundle }, "store, from delivery method");
      }

      if (kinds.size) {
        note(`game: ${[...kinds].sort().join(" + ")}`, item);
      } else if (keys.length && keys.every((k) => k.expired)) {
        note("skipped: expired keys", item);
      } else if (otherPlatforms.size) {
        note(`skipped: ${[...otherPlatforms].sort().join(" + ")}`, item);
      } else {
        const methods = deliveryMethods.filter((m) => typeof m === "string").join(", ");
        note("skipped: no keys or game downloads", item, methods ? `delivered by ${methods}` : "");
      }
    }
  }

  function toCsv(rows) {
    const cell = (v) => (/[",\r\n]/.test(v) ? `"${String(v).replace(/"/g, '""')}"` : String(v));
    const header = ["Title", "Platform", "Store", "Ownership", "Format", "Key status", "Bundle"];
    const lines = rows.map((r) =>
      [r.title, r.platform, r.store, "Owned", "Digital", r.keyStatus, r.bundle].map(cell).join(","));
    return [header.join(","), ...lines].join("\r\n");
  }

  // ---------- reading React Server Component responses ----------

  /// The JSON rows of an RSC payload (`1f:{...}`), with React's references
  /// resolved: a value repeated in a response is sent once and later spots
  /// say "$1f" (that row) or "$1f:results:0:entitlements" (a path inside it).
  /// Non-JSON rows (text, modules, hints) are skipped. With `rootsOnly`, rows
  /// that another row points to are left out, since they're already inside it.
  function parseRsc(rscText, { rootsOnly = false } = {}) {
    const referenced = new Set();
    const raw = new Map();
    for (const line of rscText.split("\n")) {
      const m = /^([0-9a-f]+):([[{"].*)$/.exec(line);
      if (!m) continue;
      try {
        raw.set(m[1], JSON.parse(m[2]));
      } catch {
        // Not a JSON row.
      }
    }

    const resolved = new Map();
    const inProgress = new Set();
    const row = (id, depth) => {
      if (resolved.has(id)) return resolved.get(id);
      if (inProgress.has(id)) return undefined; // a row that contains itself
      inProgress.add(id);
      const value = resolve(raw.get(id), depth);
      inProgress.delete(id);
      resolved.set(id, value);
      return value;
    };
    // Path references are followed through the raw rows, so they work even
    // when they point into a row that's still being resolved.
    const follow = (id, path, depth) => {
      let target = raw.get(id);
      for (const key of path) {
        if (typeof target === "string" && target[0] === "$") target = resolve(target, depth + 1);
        target = target?.[key];
      }
      return resolve(target, depth + 1);
    };
    const resolve = (v, depth = 0) => {
      if (depth > 100) return undefined; // runaway references
      if (typeof v === "string" && v[0] === "$") {
        if (v === "$undefined") return undefined;
        if (v.startsWith("$$")) return v.slice(1); // an escaped literal "$"
        const ref = /^\$@?([0-9a-f]+)((?::[^:]+)*)$/.exec(v);
        if (!ref || !raw.has(ref[1])) return v;
        referenced.add(ref[1]);
        const path = ref[2].split(":").filter(Boolean);
        return path.length ? follow(ref[1], path, depth) : row(ref[1], depth + 1);
      }
      if (Array.isArray(v)) return v.map((x) => resolve(x, depth + 1));
      if (v && typeof v === "object") {
        const out = {};
        for (const [k, x] of Object.entries(v)) out[k] = resolve(x, depth + 1);
        return out;
      }
      return v;
    };
    const all = [...raw.keys()].map((id) => [id, row(id, 0)]);
    return all.filter(([id]) => !rootsOnly || !referenced.has(id)).map(([, value]) => value);
  }

  /// Every object with an `itemType` anywhere in an RSC payload, once each.
  function findLibraryItems(rscText) {
    const found = new Map();
    const seen = new Set();
    const walk = (v) => {
      if (!v || typeof v !== "object" || seen.has(v)) return;
      seen.add(v);
      if (Array.isArray(v)) return v.forEach(walk);
      if (typeof v.itemType === "string" && v.id != null && !found.has(v.id)) found.set(v.id, v);
      Object.values(v).forEach(walk);
    };
    parseRsc(rscText).forEach(walk);
    return [...found.values()];
  }

  /// Download file types (windows, ebook, audio...) on a detail page, by the id
  /// of the library entry they're listed under. Values outside any wanted
  /// entry are filed under "*".
  function platformsById(rscText, wantedIds) {
    const found = new Map();
    const add = (owner, platform) => {
      if (!found.has(owner)) found.set(owner, new Set());
      found.get(owner).add(platform);
    };
    const walk = (v, owner) => {
      if (Array.isArray(v)) return v.forEach((x) => walk(x, owner));
      if (!v || typeof v !== "object") return;
      if (typeof v.id === "string" && wantedIds.has(v.id)) owner = v.id;
      for (const [k, x] of Object.entries(v)) {
        if (/platform|operatingsystem/i.test(k)) {
          const values = typeof x === "string" ? [x] : Array.isArray(x) ? x.filter((s) => typeof s === "string") : [];
          for (const p of values) if (KNOWN_PLATFORMS.has(p.toLowerCase())) add(owner, p.toLowerCase());
        }
        walk(x, owner);
      }
    };
    parseRsc(rscText, { rootsOnly: true }).forEach((row) => walk(row, "*"));
    return found;
  }

  /// Whether an entry has DRM-free downloads but the library list didn't say
  /// what they are (Humble leaves that to the detail pages).
  function needsFileTypes(item) {
    const downloads = item.entitlements?.downloads;
    return Array.isArray(downloads) && downloads.length > 0 &&
      downloads.every((dl) => !Array.isArray(dl?.downloads) || !dl.downloads.length);
  }

  /// The `pagination` object from a filterUserLibrary response, if any.
  function findPagination(rscText) {
    for (const line of rscText.split("\n")) {
      const i = line.indexOf('"pagination":');
      if (i < 0) continue;
      const m = /"pagination":(\{[^{}]*\})/.exec(line);
      if (m) {
        try {
          return JSON.parse(m[1]);
        } catch {
          // keep looking
        }
      }
    }
    return null;
  }

  /// The `error` code of a failed action response (`"success":false`), if any.
  function findError(rscText) {
    const m = /"success":false[^\n]*?"error":"([^"]+)"|"error":"([^"]+)"[^\n]*?"success":false/.exec(rscText);
    return m ? m[1] || m[2] : null;
  }

  // ---------- talking to humblebundle.com ----------

  async function findActionId() {
    const pattern = new RegExp(`createServerReference\\)\\("([0-9a-f]{20,})",[^)]*"${ACTION_NAME}"\\)`);
    const sources = [...document.scripts].map((s) => s.src).filter((src) => src.includes("humblebundle.com"));
    for (const src of sources) {
      try {
        const m = pattern.exec(await (await fetch(src)).text());
        if (m) return m[1];
      } catch {
        // try the next script
      }
    }
    console.warn("Humble export: couldn't find the library action in the page scripts; trying the last known one.");
    return FALLBACK_ACTION_ID;
  }

  async function withRetry(doFetch, label) {
    for (let attempt = 0; ; attempt++) {
      const resp = await doFetch();
      if (resp.status === 429 && attempt < 5) {
        await new Promise((r) => setTimeout(r, 2000 * (attempt + 1)));
        continue;
      }
      const text = await resp.text();
      if (!resp.ok) throw new Error(`${label} → HTTP ${resp.status}: ${text.slice(0, 200)}`);
      return text;
    }
  }

  /// A field name without React's part prefix: "_1_page" / "1_page" → "page".
  const fieldName = (key) => key.replace(/^_?\d+_/, "");

  /// Records the library request Humble's own page sends, by moving the page to
  /// ?page=2 (the same thing its pager does) while watching fetch. Copying that
  /// request avoids guessing how React encodes it. Returns null if the page
  /// didn't send one in time.
  async function captureLibraryRequest(timeoutMs = 10000) {
    const realFetch = window.fetch;
    let found;
    const captured = new Promise((resolve) => (found = resolve));
    window.fetch = function (input, init = {}) {
      try {
        const headers = new Headers(init.headers ?? (input instanceof Request ? input.headers : undefined));
        if (headers.has("Next-Action") && init.body instanceof FormData &&
            [...init.body.keys()].some((k) => fieldName(k) === "page")) {
          found({
            url: typeof input === "string" ? input : input.url,
            headers: Object.fromEntries(headers.entries()),
            fields: [...init.body.entries()],
          });
        }
      } catch {
        // Never break the page's own requests.
      }
      return realFetch.apply(this, arguments);
    };
    const original = location.href;
    try {
      const url = new URL(original);
      url.searchParams.set("page", url.searchParams.get("page") === "2" ? "1" : "2");
      history.pushState(null, "", url); // Next.js routes pushState like a link click
      return await Promise.race([captured, new Promise((r) => setTimeout(() => r(null), timeoutMs))]);
    } finally {
      window.fetch = realFetch;
      history.replaceState(null, "", original);
    }
  }

  /// A hand-built request, used only if capturing the page's own one fails:
  /// useActionState passes (previousState, formData), which React sends as
  /// "0" = the argument list with "$K1" pointing at the FormData's fields.
  function fallbackRequest(actionId) {
    const previous = { results: [], pagination: { page: 1, totalPages: 1, totalDocs: 0 }, success: true };
    const fields = [["0", JSON.stringify([previous, "$K1"])]];
    for (const [k, v] of Object.entries({
      requestId: "1", query: "", sort: "mostRecent", cursor: String(PAGE_SIZE), page: "1",
      limit: String(PAGE_SIZE), hideRevealed: "false", hideExpired: "false", dateExpires: "",
    })) fields.push([`_1_${k}`, v]);
    return { url: "/home/library", headers: { "Next-Action": actionId, Accept: "text/x-component" }, fields };
  }

  /// Replays a library request for another page, with every filter cleared so
  /// revealed and expired items aren't hidden by your saved library settings.
  function buildPageRequest(template, page, sort = "aToZ") {
    const limit = Number(template.fields.find(([k]) => fieldName(k) === "limit")?.[1]) || PAGE_SIZE;
    const overrides = {
      page: String(page), cursor: String(limit * page), requestId: String(page), sort,
      query: "", hideRevealed: "false", hideExpired: "false", dateExpires: "",
    };
    const body = new FormData();
    for (const [key, value] of template.fields) {
      const name = fieldName(key);
      if (name.endsWith("[]")) continue; // platform / product-type filters
      body.append(key, name in overrides && key !== "0" ? overrides[name] : value);
    }
    return { url: template.url || location.pathname, headers: template.headers, body };
  }

  async function fetchLibraryPage(template, page, sort) {
    const { url, headers, body } = buildPageRequest(template, page, sort);
    return withRetry(
      () => fetch(url, { method: "POST", credentials: "include", headers, body }),
      `library page ${page}`,
    );
  }

  /// Reads every page of the library in one sort order (Humble accepts
  /// "aToZ", "zToA" and "mostRecent") into `items`. Stops early once `items`
  /// holds `stopAt` entries. Returns Humble's total entry count.
  async function readLibrary(template, sort, items, stopAt = Infinity) {
    let totalPages = 1;
    let totalDocs;
    let previousIds = "";
    for (let page = 1; page <= Math.min(totalPages, MAX_PAGES); page++) {
      const text = await fetchLibraryPage(template, page, sort);
      const error = findError(text);
      if (error === "not_authenticated") throw new Error("You're not signed in. Sign in at humblebundle.com, reload the library page and run this again.");
      if (error) throw new Error(`Humble's library answered "${error}" on page ${page}.`);
      const pageItems = findLibraryItems(text);
      const pagination = findPagination(text);
      if (page === 1) {
        if (!pagination && !pageItems.length) {
          console.log("Humble export: unexpected first response:\n" + text.slice(0, 1500));
          throw new Error("Humble's library didn't answer as expected. Are you signed in? Copy the console output into an issue.");
        }
        totalDocs = pagination?.totalDocs;
        console.log(`Humble export: ${totalDocs ?? "?"} library entries on ${pagination?.totalPages ?? "?"} pages (sorted ${sort})`);
      }
      // The same results twice means Humble ignored the page number.
      const ids = pageItems.map((it) => it.id).join(",");
      if (page > 1 && ids && ids === previousIds) {
        console.log("Humble export: request used:", template.fields);
        throw new Error(`Page ${page} returned the same entries as page ${page - 1}, so paging isn't working. ` +
          "Copy the console output into an issue.");
      }
      previousIds = ids;
      totalPages = pagination?.totalPages ?? (pageItems.length ? page + 1 : page);
      pageItems.forEach((it) => items.set(it.id, it));
      if (items.size >= stopAt) break;
      if (page % 20 === 0) console.log(`Humble export: page ${page}/${totalPages}`);
    }
    return totalDocs;
  }

  /// Runs `task` over `inputs`, `limit` at a time.
  async function pool(inputs, limit, task) {
    let next = 0;
    const worker = async () => {
      while (next < inputs.length) {
        const input = inputs[next++];
        await task(input);
      }
    };
    await Promise.all(Array.from({ length: Math.min(limit, inputs.length) }, worker));
  }

  const fetchDetail = (path) => withRetry(
    () => fetch(path, { credentials: "include", headers: { RSC: "1" } }),
    path,
  );

  /// Fills in download file types for entries the library list left blank:
  /// first from each purchase's page (one request covers a whole bundle), then
  /// from the entry's own page for anything still unknown. Sets
  /// `downloads[].downloads = [{platform}, ...]` like Humble's full data.
  async function lookUpFileTypes(items) {
    const pending = items.filter(needsFileTypes);
    if (!pending.length) return;
    const known = new Map(); // item id → Set of platforms
    const purchases = new Map(); // purchase id → item ids
    for (const it of pending) {
      const purchase = typeof it.productEntitlementId === "string" ? it.productEntitlementId : it.id;
      if (!purchases.has(purchase)) purchases.set(purchase, []);
      purchases.get(purchase).push(it.id);
    }
    console.log(`Humble export: looking up file types for ${pending.length} download entries from ${purchases.size} purchases…`);

    let done = 0;
    await pool([...purchases], 4, async ([purchase, ids]) => {
      try {
        const found = platformsById(await fetchDetail(`/home/library/${encodeURIComponent(purchase)}`), new Set(ids));
        for (const id of ids) if (found.has(id)) known.set(id, found.get(id));
        // A purchase of a single item may list its files without repeating the id.
        if (ids.length === 1 && !known.has(ids[0]) && found.has("*")) known.set(ids[0], found.get("*"));
      } catch (err) {
        console.warn(`Humble export: couldn't read purchase ${purchase}: ${err.message}`);
      }
      if (++done % 25 === 0) console.log(`Humble export: ${done}/${purchases.size} purchases checked`);
    });

    const leftovers = pending.filter((it) => !known.has(it.id));
    if (leftovers.length) console.log(`Humble export: checking ${leftovers.length} entries one by one…`);
    done = 0;
    await pool(leftovers, 4, async (it) => {
      try {
        const found = platformsById(await fetchDetail(`/home/library/content/${encodeURIComponent(it.id)}`), new Set([it.id]));
        const platforms = found.get(it.id) ?? found.get("*");
        if (platforms) known.set(it.id, platforms);
      } catch (err) {
        console.warn(`Humble export: couldn't read "${it.title}": ${err.message}`);
      }
      if (++done % 50 === 0) console.log(`Humble export: ${done}/${leftovers.length} entries checked`);
    });

    for (const it of pending) {
      const platforms = known.get(it.id);
      if (!platforms) continue;
      it.entitlements.downloads[0] = { ...it.entitlements.downloads[0], downloads: [...platforms].map((platform) => ({ platform })) };
    }
    console.log(`Humble export: found file types for ${pending.filter((it) => known.has(it.id)).length} of ${pending.length} download entries`);
  }

  async function fetchGroup(id) {
    return withRetry(
      () => fetch(`/home/library/group/${encodeURIComponent(id)}`, { credentials: "include", headers: { RSC: "1" } }),
      `bundle ${id}`,
    );
  }

  /// Console tables of what happened to every library entry, games first.
  function printBreakdown(breakdown, notReturned) {
    const table = (prefix) => Object.fromEntries(
      [...breakdown]
        .filter(([category]) => category.startsWith(prefix))
        .sort(([, a], [, b]) => b.count - a.count)
        .map(([category, { count, examples }]) => [category.slice(prefix.length), { entries: count, examples: examples.join(" · ") }]),
    );
    console.log("Humble export: library entries that became games");
    console.table(table("game: "));
    console.log("Humble export: library entries left out");
    console.table({
      ...table("skipped: "),
      ...(notReturned ? { "not returned by Humble": { entries: notReturned, examples: "Humble's total count was higher than what its pages listed" } } : {}),
    });
  }

  /// Prints raw download-only entries and what their likely detail pages contain,
  /// so the export can learn where Humble keeps download file types.
  async function diagnoseDownloads(items) {
    const isUnclassified = (it) => it.itemType !== "entitlement-group" &&
      !(Array.isArray(it.entitlements?.keys) && it.entitlements.keys.length) &&
      Array.isArray(it.deliveryMethods) && it.deliveryMethods.includes("download");
    const samples = items.filter(isUnclassified).slice(0, 3);
    console.log(`Humble export [diagnose]: ${items.filter(isUnclassified).length} download-only entries in these pages.`);
    // Keys, download links and checksums are blanked: this output is meant to be shared.
    const redact = (k, v) => {
      if (v != null && /key|url|token|sig|md5|sha|email|address|torrent/i.test(k) && k !== "keys") return "[redacted]";
      if (typeof v === "string") return v.replace(/https?:\/\/\S*\?\S*/g, "[redacted link]");
      return v;
    };
    samples.forEach((it, i) => console.log(`Humble export [diagnose]: sample ${i + 1}:\n` + JSON.stringify(it, redact, 1).slice(0, 3000)));

    const it = samples[0];
    if (!it) return;
    const ids = [...new Set([it.id, it.productEntitlementId, it.entitlements?.downloads?.[0]?.id].filter((x) => typeof x === "string"))];
    const urls = ids.flatMap((id) => [`/home/library/content/${id}`, `/home/library/${id}`]);
    for (const url of urls) {
      try {
        const resp = await fetch(url, { credentials: "include", headers: { RSC: "1" } });
        const text = await resp.text();
        const platforms = new Set();
        const walk = (v) => {
          if (Array.isArray(v)) return v.forEach(walk);
          if (v && typeof v === "object") {
            for (const [k, x] of Object.entries(v)) {
              if (typeof x === "string" && /platform/i.test(k) && KNOWN_PLATFORMS.has(x.toLowerCase())) platforms.add(`${k}=${x}`);
              walk(x);
            }
          }
        };
        parseRsc(text).forEach(walk);
        const keys = [...new Set([...text.matchAll(/"(downloads?|downloadStruct|platforms?|operatingSystems|fileType|format)"/g)].map((m) => m[1]))];
        console.log(`Humble export [diagnose]: ${url} → HTTP ${resp.status}, ${text.length} bytes, ` +
          `platform values: ${[...platforms].join(", ") || "none"}; keys seen: ${keys.join(", ") || "none"}`);
      } catch (err) {
        console.log(`Humble export [diagnose]: ${url} → ${err.message}`);
      }
    }
  }

  async function run() {
    if (!location.hostname.endsWith("humblebundle.com")) {
      throw new Error("Open https://www.humblebundle.com/home/library while signed in, then run this again.");
    }
    if (!location.pathname.startsWith("/home/library")) {
      throw new Error("Open https://www.humblebundle.com/home/library (your library page), then run this again.");
    }
    let template = await captureLibraryRequest();
    if (template) {
      console.log(`Humble export: using the library page's own request (fields: ${template.fields.map(([k]) => k).join(", ")})`);
    } else {
      console.warn("Humble export: the library page didn't load a page of results to copy; building the request by hand.");
      template = fallbackRequest(await findActionId());
    }

    // Humble's pages can overlap, repeating some entries and skipping others.
    // Alphabetical order pages most consistently; a second pass in another
    // order picks up anything still missing.
    const items = new Map();
    const totalDocs = await readLibrary(template, "aToZ", items);
    if (!DIAGNOSE && typeof totalDocs === "number" && items.size < totalDocs) {
      console.log(`Humble export: ${totalDocs - items.size} entries weren't listed; reading again in another order…`);
      await readLibrary(template, "mostRecent", items, totalDocs);
    }
    console.log(`Humble export: read ${items.size} of ${totalDocs ?? "?"} library entries` +
      (DIAGNOSE ? ` (diagnostic mode reads only the first ${MAX_PAGES} pages)` : ""));
    const notReturned = !DIAGNOSE && typeof totalDocs === "number" ? Math.max(0, totalDocs - items.size) : 0;

    // Bundles list their games on their own page.
    const bundleOf = new Map();
    const groups = [...items.values()].filter((it) => it.itemType === "entitlement-group");
    let groupFailures = 0;
    for (const [i, group] of groups.entries()) {
      try {
        for (const it of findLibraryItems(await fetchGroup(group.id))) {
          if (it.id === group.id) continue;
          items.set(it.id, it);
          if (!bundleOf.has(it.id)) bundleOf.set(it.id, group.title || "");
        }
      } catch (err) {
        groupFailures++;
        console.warn(`Humble export: couldn't read bundle "${group.title}": ${err.message}`);
      }
      if ((i + 1) % 10 === 0) console.log(`Humble export: ${i + 1}/${groups.length} bundles read`);
    }

    // The library list doesn't say whether a download is a game or a book.
    await lookUpFileTypes([...items.values()]);

    const breakdown = new Map();
    const rows = itemsToRows([...items.values()], bundleOf, breakdown);
    printBreakdown(breakdown, notReturned);
    if (DIAGNOSE) {
      await diagnoseDownloads([...items.values()]);
      console.log("Humble export [diagnose]: done; no CSV saved in diagnostic mode.");
      return;
    }
    if (!rows.length) {
      console.log("Humble export: sample item:", [...items.values()][0]);
      throw new Error("Found library entries but no games in them. Copy the console output into an issue.");
    }
    const blob = new Blob(["﻿" + toCsv(rows)], { type: "text/csv;charset=utf-8" });
    const a = Object.assign(document.createElement("a"), {
      href: URL.createObjectURL(blob),
      download: `humble-library-${new Date().toISOString().slice(0, 10)}.csv`,
    });
    document.body.append(a);
    a.click();
    a.remove();
    console.log(`Humble export: saved ${rows.length} games from ${items.size} library entries and ${groups.length} bundles` +
      (groupFailures ? ` (${groupFailures} bundles couldn't be read; see warnings above)` : "") + ".");
  }

  if (typeof window === "undefined") {
    module.exports = {
      itemsToRows, toCsv, cleanTitle, findLibraryItems, findPagination, findError, buildPageRequest, fieldName, parseRsc,
      printBreakdown, platformsById, needsFileTypes, readLibrary,
    };
  } else {
    run().catch((err) => console.error(`Humble export failed: ${err.message}`));
  }
})();
