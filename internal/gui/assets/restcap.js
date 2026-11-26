// A provider editor's Failure rest row under Advanced (provider.RestCap,
// the CLI's `provider rest-cap`): how long one of its keys or accounts
// sits out after a failure that says nothing of why, a relay's 5xx or a
// dropped connection. Default is the gateway's own (a minute, doubling on
// each failure in a row, at most 10; rate limited, at most 30). No rest
// asks it again on the next request (-1). Custom caps the doubling, rate
// limits included, at the minutes typed; left empty it is the default.
// draft.restCap is set only once the row is changed, so a save that never
// touched it keeps what the CLI set. app.js's concurrencyField imports
// this, so it weighs nothing on the core.
(() => {
  const HINT = {
    "": "After a failure that says nothing of why (a 5xx, a dropped connection), a key or account sits out a minute, twice as long for each failure in a row, at most 10 minutes; rate limited, at most 30",
    off: "The next request is sent to it as usual after such a failure. A refused key (401, 403), quota or credit used up, a rate limit or a model it doesn't serve still rests it",
    custom: "As the default, but a key or account sits out at most the minutes set here, rate limited too; empty is the default",
  };
  window.restCapRow = function (at, p, d) {
    const was = p?.restCap || 0;
    const mins = input(was > 0 ? String(was) : "", "10", "number");
    mins.classList.add("rest-cap-min");
    Object.assign(mins, { min: "1", max: "11520", step: "1", inputMode: "numeric" });
    const custom = el("span", "rest-cap-custom");
    custom.append(mins, el("span", "hint", t("min")));
    // a whole number of minutes, held to what the save takes (1 to 8
    // days); empty or 0 is the default
    const typed = () => Math.min(Math.max(Math.round(Number(mins.value)) || 0, 0), 11520);
    const mode = was < 0 ? "off" : was > 0 ? "custom" : "";
    const row = el("div", "rest-cap");
    const seg = segs([["", t("Default")], ["off", t("No rest")], ["custom", t("Custom")]], mode, (v) => {
      d.restCap = v === "off" ? -1 : v === "custom" ? typed() : 0;
      place(v);
      if (v === "custom") mins.focus();
    });
    seg.classList.add("rest-cap-mode");
    row.append(seg, custom);
    mins.oninput = () => { d.restCap = typed(); };
    const f = field(t("Failure rest"), row, t(HINT[mode]));
    const hint = f[1].querySelector(".hint.expl");
    const place = (v) => {
      custom.classList.toggle("off", v !== "custom");
      hint.textContent = t(HINT[v]);
    };
    place(mode);
    at.replaceWith(...f);
  };
})();
