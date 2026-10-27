// Builds the atomik-kb:// link from the address of this page. The secret of the invitation is the
// fragment (after the #): it never reaches the server, and this script sends it nowhere.
(function () {
  "use strict";
  var link = document.getElementById("open");
  var message = document.getElementById("message");
  var secret = window.location.hash.replace(/^#/, "");
  if (!/^[0-9A-Za-z-]{40,100}$/.test(secret)) {
    message.textContent =
      "This invitation link is incomplete. Ask the person who invited you for a new one.";
    return;
  }
  var base = window.location.origin + window.location.pathname.replace(/\/join\/?$/, "");
  link.href = "atomik-kb://invite?server=" + encodeURIComponent(base) + "#" + secret;
  link.hidden = false;
})();
