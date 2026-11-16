// "Copy page as Markdown": puts the page's Markdown twin (written beside it by
// `tools/postbuild.mjs`) on the clipboard. The button ships `hidden` and is
// shown only where the Clipboard API exists, so a reader without it keeps the
// plain "View as Markdown" link and never meets a button that cannot work.
(function () {
  const button = document.querySelector(".doc-md-copy");
  if (!button || !navigator.clipboard) return;
  const status = button.parentElement.querySelector(".doc-md-status");
  button.hidden = false;

  const text = () =>
    fetch(button.dataset.twin).then((res) => {
      if (!res.ok) throw new Error(res.status);
      return res.text();
    });

  button.addEventListener("click", async () => {
    status.textContent = "";
    try {
      // Safari drops the user activation across an `await fetch`, so hand the
      // clipboard a pending item where it accepts one and fetch inside it.
      if (window.ClipboardItem && navigator.clipboard.write) {
        const blob = text().then((t) => new Blob([t], { type: "text/plain" }));
        await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
      } else {
        await navigator.clipboard.writeText(await text());
      }
      status.textContent = button.dataset.done;
    } catch (e) {
      status.textContent = button.dataset.fail;
    }
  });
})();
