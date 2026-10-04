# Changelog

## 0.2.1

- **Sidebar starts closed.** More room for the list; open it with ☰ or ⌘B. New **Localhost / Projects** tabs in the top bar.
- **Bigger ▶ / ■ buttons**, always visible, green to start and red to stop.
- Back from 0.1.2: projects sitting straight in a folder group under that folder (e.g. **CODE**), and a running project
  that answers HTTP 5xx gets a red dot in the Projects list.
- The top bar no longer wraps on narrower windows.

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
