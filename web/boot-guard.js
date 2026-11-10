/* Last-resort guard: if the module app never boots, replace the spinner with
   an actionable message instead of loading forever. */
(function () {
  window.setTimeout(function () {
    var boot = document.getElementById("boot");
    if (!boot || !boot.isConnected) return;

    var card = document.createElement("div");
    card.className = "boot-error";

    var title = document.createElement("h2");
    title.textContent = "Sethu could not finish loading";
    var detail = document.createElement("p");
    detail.textContent =
      "Reload this page. If it keeps happening, open the site in a private tab or clear this site's cache.";
    var button = document.createElement("button");
    button.type = "button";
    button.className = "btn btn-primary";
    button.textContent = "Reload";
    button.addEventListener("click", function () {
      window.location.reload();
    });

    card.append(title, detail, button);
    boot.replaceChildren(card);
  }, 12000);
})();
