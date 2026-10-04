# Changelog

## 0.2.0

- **Running servers only by default.** The project list and the sidebar show just what is running; tick **Show all**
  to see every project, stopped ones included. Server Hub remembers your choice.
- **Share on your local network (⇄).** Open a dev server from your phone or another computer on the same Wi-Fi.
  Servers that already listen on every address get their LAN link right away; servers bound to `127.0.0.1` only
  (vite, next dev…) get a small relay on `<port + 10000>`, e.g. `5173 → http://192.168.1.20:15173`. The link is copied
  for you; click ⇄ twice to stop sharing. Relays close when the server stops or you quit Server Hub.
- **Open source** under the MIT license.

## 0.1.1

- Drag the window from the top bar; scan several project folders.

## 0.1.0

- First public release: Localhost view with health checks, Projects view with ▶ / ■ and live logs, menu bar icon.
