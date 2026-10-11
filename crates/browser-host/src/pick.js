// Neptune element picker: one expression, evaluated in the page's main world.
// Resolves exactly once with a JSON string describing the clicked element, or
// null when the pick was cancelled. Removes everything it added before resolving.
(() => new Promise((resolve) => {
  const CANCEL = Symbol.for("neptune.pick.cancel");
  const ACCENT = "#4c8dff";
  const PILL =
    "position:fixed;display:none;box-sizing:border-box;padding:3px 8px;border-radius:6px;" +
    "background:rgba(24,24,27,.92);color:#fff;font:12px/16px system-ui,sans-serif;" +
    "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:calc(100vw - 8px);";
  const PILL_HEIGHT = 22; // 16px line + 2 * 3px padding
  const BLOCKED = ["pointerdown", "mousedown", "pointerup", "mouseup", "click", "auxclick", "contextmenu"];
  const SELECTOR_ATTRS = [
    "data-testid", "data-test-id", "data-test", "data-cy", "data-qa", "aria-label",
    "name", "href", "src", "role", "title", "alt",
  ];
  const STYLE_PROPS = [
    "display", "position", "width", "height", "margin", "padding", "color", "background-color",
    "font-family", "font-size", "font-weight", "line-height", "border", "border-radius", "gap",
    "flex-direction", "justify-content", "align-items", "opacity", "z-index",
  ];
  const FLEX_ONLY = new Set(["gap", "flex-direction", "justify-content", "align-items"]);
  const STYLE_ALWAYS = new Set(["display", "width", "height", "color", "font-size"]);
  const STYLE_DEFAULTS = new Set(["", "none", "normal", "auto", "0px", "static", "rgba(0, 0, 0, 0)"]);

  // Only one pick at a time: a second evaluation replaces the first.
  try {
    if (typeof window[CANCEL] === "function") window[CANCEL]();
  } catch {}

  // ---- overlay -----------------------------------------------------------
  // Shadow parts are styled through CSSOM (no <style>, no innerHTML) so page
  // CSS, strict style-src CSP and Trusted Types cannot break the overlay.
  const host = document.createElement("div");
  host.style.cssText =
    "position:fixed!important;inset:0!important;z-index:2147483646!important;" +
    "pointer-events:none!important;display:block!important;margin:0!important;" +
    "padding:0!important;border:0!important;background:none!important;";
  const shadow = host.attachShadow({ mode: "closed" });
  const box = document.createElement("div");
  box.style.cssText =
    `position:fixed;display:none;box-sizing:border-box;border:2px solid ${ACCENT};` +
    "background:rgba(76,141,255,.1);border-radius:3px;";
  const label = document.createElement("div");
  label.style.cssText = PILL;
  const hint = document.createElement("div");
  hint.style.cssText = PILL + "display:block;top:10px;left:50%;transform:translateX(-50%);";
  hint.textContent = "Click an element to copy it \u00b7 Esc to cancel";
  shadow.append(box, label, hint);

  const cursorStyle = document.createElement("style");
  cursorStyle.textContent = "*, *::before, *::after { cursor: crosshair !important; }";

  let active = true;
  let pointerX = -1;
  let pointerY = -1;

  // ---- helpers -----------------------------------------------------------
  // Cut to `max` UTF-16 units without leaving half a surrogate pair behind:
  // a lone surrogate makes strict JSON parsers on the host side reject the pick.
  const clip = (value, max, ellipsis = false) => {
    let s = typeof value === "string" ? value : value == null ? "" : String(value);
    if (s.length > max) {
      s = s.slice(0, ellipsis ? max - 1 : max);
      const last = s.charCodeAt(s.length - 1);
      if (last >= 0xd800 && last <= 0xdbff) s = s.slice(0, -1);
      if (ellipsis) s += "\u2026";
    }
    return typeof s.toWellFormed === "function" ? s.toWellFormed() : s;
  };
  const attempt = (fn, fallback) => {
    try {
      const value = fn();
      return value === undefined ? fallback : value;
    } catch {
      return fallback;
    }
  };
  const round = (n) => Math.round(n * 100) / 100;
  const collapse = (s) => String(s ?? "").replace(/\s+/g, " ").trim();

  const targetAt = (x, y) => {
    for (const el of document.elementsFromPoint(x, y)) {
      if (el !== host && el !== document.documentElement && el !== document.body) return el;
    }
    return null;
  };

  const visibleRect = (el) => {
    const r = el.getBoundingClientRect();
    const left = Math.max(0, r.left);
    const top = Math.max(0, r.top);
    const right = Math.min(window.innerWidth, r.right);
    const bottom = Math.min(window.innerHeight, r.bottom);
    return { x: left, y: top, width: Math.max(0, right - left), height: Math.max(0, bottom - top) };
  };

  const describe = (el) => {
    let text = el.localName;
    if (el.id) text += "#" + el.id;
    for (const name of [...el.classList].slice(0, 2)) text += "." + name;
    return text;
  };

  const highlight = () => {
    const el = pointerX < 0 ? null : attempt(() => targetAt(pointerX, pointerY), null);
    if (!el) {
      box.style.display = label.style.display = "none";
      return;
    }
    const r = visibleRect(el);
    box.style.display = "block";
    box.style.left = r.x + "px";
    box.style.top = r.y + "px";
    box.style.width = r.width + "px";
    box.style.height = r.height + "px";
    label.textContent = clip(describe(el), 160);
    label.style.display = "block";
    // Above the box when it fits, otherwise tucked inside its top edge.
    const above = r.y - PILL_HEIGHT - 4;
    label.style.top = (above >= 0 ? above : r.y + 4) + "px";
    label.style.left = Math.max(4, Math.min(r.x, window.innerWidth - label.offsetWidth - 4)) + "px";
  };

  // ---- selector ----------------------------------------------------------
  const quote = (value) =>
    '"' +
    value
      .replace(/["\\]/g, "\\$&")
      .replace(/[\0-\x1f\x7f]/g, (c) => "\\" + c.charCodeAt(0).toString(16) + " ") +
    '"';
  const matchesOnly = (selector, el) =>
    attempt(() => {
      const found = document.querySelectorAll(selector);
      return found.length === 1 && found[0] === el;
    }, false);
  const uniqueId = (el) => {
    const id = el.id;
    if (!id || !/^[A-Za-z_][\w-]*$/.test(id)) return null;
    const selector = "#" + CSS.escape(id);
    return matchesOnly(selector, el) ? selector : null;
  };
  const pathSegment = (el) => {
    const tag = CSS.escape(el.localName);
    const sameTag = el.parentElement
      ? [...el.parentElement.children].filter((s) => s.localName === el.localName)
      : [el];
    const name = [...el.classList].find((c) => c.length <= 40 && /^[a-zA-Z][\w-]*$/.test(c));
    if (name && sameTag.filter((s) => s.classList.contains(name)).length === 1) {
      return tag + "." + CSS.escape(name);
    }
    return `${tag}:nth-of-type(${sameTag.indexOf(el) + 1})`;
  };
  const selectorFor = (el) => {
    const byId = uniqueId(el);
    if (byId) return byId;
    for (const attr of SELECTOR_ATTRS) {
      const value = el.getAttribute(attr);
      if (value == null || value.length < 1 || value.length > 120) continue;
      const bare = `[${attr}=${quote(value)}]`;
      if (matchesOnly(bare, el)) return bare;
      const tagged = CSS.escape(el.localName) + bare;
      if (matchesOnly(tagged, el)) return tagged;
    }
    const parts = [];
    for (let node = el; node; node = node.parentElement) {
      if (node === document.body || node === document.documentElement) {
        parts.unshift(node.localName);
        break;
      }
      const anchor = node === el ? null : uniqueId(node);
      if (anchor) {
        parts.unshift(anchor);
        break;
      }
      parts.unshift(pathSegment(node));
      if (matchesOnly(parts.join(" > "), el)) break;
    }
    return parts.join(" > ");
  };

  // ---- computed styles ---------------------------------------------------
  const stylesFor = (el) => {
    const computed = getComputedStyle(el);
    const lines = [];
    for (const prop of STYLE_PROPS) {
      const value = computed.getPropertyValue(prop).trim();
      const isDefault = STYLE_DEFAULTS.has(value) || (prop === "opacity" && value === "1");
      if (isDefault && !STYLE_ALWAYS.has(prop)) continue;
      // A border of no width and a layout the element does not use say nothing.
      if (prop === "border" && value.startsWith("0px")) continue;
      if (FLEX_ONLY.has(prop) && !/flex|grid/.test(computed.display)) continue;
      lines.push(`${prop}: ${value};`);
    }
    return lines.join("\n");
  };

  // ---- React -------------------------------------------------------------
  const componentName = (type, depth = 0) => {
    if (!type || depth > 4) return "";
    if (typeof type === "function") return type.displayName || type.name || "";
    if (typeof type !== "object") return "";
    return (
      type.displayName ||
      componentName(type.render, depth + 1) || // forwardRef
      componentName(type.type, depth + 1) || // memo
      ""
    );
  };
  const reactInfo = (el) => {
    const info = { component: null, owners: [], source: null };
    const key = Object.keys(el).find(
      (k) => k.startsWith("__reactFiber$") || k.startsWith("__reactInternalInstance$"),
    );
    let fiber = key ? el[key] : null;
    // The step bound guards against a cyclic or absurdly deep `return` chain.
    for (let steps = 0; fiber && steps < 500 && info.owners.length < 4; steps += 1, fiber = fiber.return) {
      const src = fiber._debugSource;
      if (!info.source && src && src.fileName) {
        const parts = [src.fileName, src.lineNumber, src.columnNumber].filter((p) => p != null);
        info.source = clip(parts.join(":"), 512);
      }
      const name = attempt(() => componentName(fiber.type), "");
      if (typeof name !== "string" || !name || /^[a-z]/.test(name)) continue;
      if (info.component === null) info.component = clip(name, 128);
      else info.owners.push(clip(name, 128));
    }
    return info;
  };

  const collect = (el) => {
    const react = attempt(() => reactInfo(el), { component: null, owners: [], source: null });
    const rect = attempt(() => visibleRect(el), { x: 0, y: 0, width: 0, height: 0 });
    return {
      url: clip(attempt(() => location.href, ""), 2048),
      title: clip(attempt(() => document.title, ""), 256),
      tag: clip(attempt(() => el.localName.toLowerCase(), ""), 64),
      selector: clip(attempt(() => selectorFor(el), ""), 512),
      text: clip(attempt(() => collapse(el.innerText ?? el.textContent), ""), 200),
      html: clip(attempt(() => el.outerHTML.replace(/\s+/g, " "), ""), 2000, true),
      styles: clip(attempt(() => stylesFor(el), ""), 2000),
      component: react.component,
      owners: react.owners,
      source: react.source,
      rect: { x: round(rect.x), y: round(rect.y), width: round(rect.width), height: round(rect.height) },
      dpr: attempt(() => window.devicePixelRatio, 1),
    };
  };

  // ---- lifecycle ---------------------------------------------------------
  const onMove = (event) => {
    pointerX = event.clientX;
    pointerY = event.clientY;
    highlight();
  };
  const onKey = (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopImmediatePropagation();
    cancel();
  };
  const onBlocked = (event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
    if (event.type !== "click" || event.button !== 0) return;
    const el = attempt(() => targetAt(event.clientX, event.clientY), null);
    if (el) pick(el); // a click on bare <body> keeps the picker open
  };
  const capture = { capture: true, passive: false };

  const teardown = () => {
    active = false;
    for (const type of BLOCKED) window.removeEventListener(type, onBlocked, capture);
    window.removeEventListener("pointermove", onMove, true);
    window.removeEventListener("scroll", highlight, true);
    window.removeEventListener("resize", highlight, true);
    window.removeEventListener("keydown", onKey, true);
    window.removeEventListener("pagehide", cancel, true);
    host.remove();
    cursorStyle.remove();
    if (window[CANCEL] === cancel) delete window[CANCEL];
  };
  function cancel() {
    if (!active) return;
    teardown();
    resolve(null);
  }
  const pick = (el) => {
    if (!active) return;
    let json;
    try {
      json = JSON.stringify(collect(el));
    } catch {
      json = "{}";
    }
    teardown();
    // Give the host two painted frames plus a margin to capture the page
    // without the overlay. requestAnimationFrame never fires in a hidden
    // document, so a timer makes sure the pick still resolves.
    let settled = false;
    const settle = () => {
      if (settled) return;
      settled = true;
      setTimeout(() => resolve(json), 120);
    };
    requestAnimationFrame(() => requestAnimationFrame(settle));
    setTimeout(settle, 500);
  };

  for (const type of BLOCKED) window.addEventListener(type, onBlocked, capture);
  window.addEventListener("pointermove", onMove, true);
  window.addEventListener("scroll", highlight, true);
  window.addEventListener("resize", highlight, true);
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("pagehide", cancel, true);
  Object.defineProperty(window, CANCEL, { value: cancel, configurable: true, enumerable: false, writable: true });
  document.documentElement.append(host, cursorStyle);
}))()
