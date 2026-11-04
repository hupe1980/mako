// A screenshot in the docs is a link to its full-size image. This turns the
// click into an enlarged view in a <dialog> — Esc, a click or the back of the
// dialog closes it — and leaves the plain link working wherever it does not run.
document.addEventListener("click", function (event) {
  var link = event.target.closest && event.target.closest(".doc-body a");
  if (!link || !link.querySelector("img") || !/\.(png|jpe?g|webp)$/i.test(link.getAttribute("href") || "")) return;
  if (typeof HTMLDialogElement !== "function" || event.metaKey || event.ctrlKey) return;
  event.preventDefault();
  var dialog = document.createElement("dialog");
  dialog.className = "zoom";
  var image = document.createElement("img");
  image.src = link.href;
  image.alt = link.querySelector("img").alt;
  dialog.appendChild(image);
  dialog.addEventListener("click", function () { dialog.close(); });
  dialog.addEventListener("close", function () { dialog.remove(); link.focus(); });
  document.body.appendChild(dialog);
  dialog.showModal();
});
