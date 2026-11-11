/* The pairing confirmation page auto-submits after the browser loads it.
   Link previews do not execute this, so they cannot consume the token. */
(function () {
  var form = document.getElementById("pair-form");
  if (form) form.submit();
})();
