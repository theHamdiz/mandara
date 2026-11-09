# Connected plugins

Translator, Phone Connect, Feeds, Reddit and Home Assistant are bundled with
Mandara and start switched off. Turn them on in Studio → Plugins; open
a plugin’s Settings to configure it. They load data when you open their
panels. Closing a panel stops its refresh timer.

## Translator

Choose **DeepL Free**, **DeepL Pro** or **LibreTranslate**. DeepL uses its
official API server and needs your own API key. LibreTranslate uses the
server address you enter; enable **Use an API key** only if that server
requires one. Addresses can include a reverse proxy path.

API keys require HTTPS for remote servers. For a local HTTP server, use
`localhost` or its private IP address; other hostnames require HTTPS. Local
HTTP does not encrypt API keys or Home Assistant tokens, so use it only
on a trusted network.

Save the key in the plugin’s masked keyring field. Keys live in your
desktop’s Secret Service keyring, scoped to the provider and server,
and stay out of Mandara’s configuration and request URLs. Removing a key
removes only that provider’s key for that server. A locked keyring is
reported without opening an unlock prompt just because a panel opened. Key
changes finish one at a time, even if settings are closed and reopened.

Choose source and target languages from your provider’s current list.
**Detect automatically** leaves the source to the provider. Type or explicitly
paste text, then press **Translate**. Typing never submits text, and
copying a result needs an explicit click. Text and results stay in memory.
A translation can contain up to 5,000 characters. Your selected provider
receives the submitted text; its account limits and privacy policy apply.

## Phone Connect

Install KDE Connect from your distribution and its companion app on your
phone. Open KDE Connect once if your desktop has not started its daemon.
The package is `kdeconnect` on Arch and Debian, and `kde-connect` on Fedora.
KDE Connect is optional and is not installed for people who do not use it.

Open Phone Connect to see the devices the local KDE Connect daemon knows
about. Pairing requires confirmation; incoming requests offer **Accept**
and **Reject**. A paired, reachable device can report its real battery
level and offer **Ring**, **Ping** or **Send clipboard text**, depending
on the device’s enabled capabilities. Clipboard text is read and sent only
when you press that button. **Forget device** asks for confirmation.

This complements Share to phone’s one-time QR transfers: it uses an
existing paired KDE Connect connection rather than creating a web share.

## Feeds

Add up to 16 RSS, Atom or JSON feed addresses, separated by spaces.
The selected feed refreshes while the panel is open; the interval is
configurable from one minute to one hour. Conditional requests reuse
ETags and modification dates. Headlines and read flags are cached locally
so the last successful result remains available offline.
Redirects are not followed. If a subscription redirects, enter its final
feed address in the plugin's Settings.

Click a headline to open its web address in your browser. It is marked
read after the browser opens successfully. A feed keeps its latest 100
entries, and read flags are kept for those entries. Subscriptions and
read history stay on this computer.

## Reddit

Read your communities, their posts and comment threads in a panel.

**Two modes.** Reddit stopped answering anonymous `.json` requests in
2026 (they return 403). The plugin works either way:

- **Limited mode** needs nothing: it reads Reddit's public Atom feeds
  (`/r/<name>/<sort>/.rss` and each post's comments feed). Reddit allows
  about one anonymous feed request a minute, so Mandara asks at most once
  a minute for the whole shell, waits longer when Reddit's
  `X-Ratelimit-*` headers or a 429 `Retry-After` say so, and never sends
  a burst. One queue serves the panel and the sidebar card: the view you
  asked for last goes first, and an older waiting request is replaced
  instead of piling up. Every feed is kept five minutes. The feeds carry
  titles, authors, links, times and the post or comment text (a picture
  post links to its picture, which shows); they carry no scores, comment
  counts or reply structure, so those are left out and comments read as
  one list. A banner says so, with the steps to full mode.
- **Full mode** reads Reddit's official OAuth API with a free app you
  create once:

1. Open [reddit.com/prefs/apps](https://www.reddit.com/prefs/apps) and
   choose *create another app*.
2. Pick **installed app**, give it any name and use `http://localhost` as
   the redirect address.
3. Copy the id shown under the app's name into Studio → Plugins → Reddit
   → **Client id**.

Installed apps have no secret, and Mandara never signs in as you: it asks
for an application-only token (Reddit's documented `installed_client`
grant, with the anonymous device id), keeps it in memory for at most an
hour and never writes it to disk. This grant has no refresh token: a
new token is simply requested a minute before the old one expires, and
once more if Reddit answers 401, with nothing for you to do. If Reddit
refuses the client id, the plugin switches to limited mode until you
change it; if a renewal fails for another reason, that request reads the
public feeds and the app is tried again next time, so you never get an
error wall. Reddit may ask new apps to be approved under its Responsible
Builder Policy. The optional **Your Reddit user name** only goes into the
request's user agent, as Reddit asks of developers.

**Opening it.** Add Reddit to a bar in Studio → Bars for a bar button, or
type `reddit`, `subreddit` or `r/linux` in the launcher
(`mandara msg reddit open r/linux`); without a bar button the panel
opens as a floating card.

**Browsing.** Your communities sit along the top (add, remove and
reorder them in Discover, which also has a starter list by topic). Sort
by hot, new, top rated (with its time range) or rising, search within a
community, and load more pages as you reach the end of the list; posts
already shown are never fetched again and never repeated. Cards show the
title, score, comment count, flair, author and age with a thumbnail;
the compact layout fits more, and the grid puts pictures first in two
or three columns (the panel widens for it). The layout button in the
panel's header switches between them, and remembers your choice. Opening a post shows its picture or
gallery, its link or a button for videos (which play in your browser),
its text and its comments: fold a reply chain by its header, load hidden
replies and continue long threads in place. Every post can be opened in
the browser, and its link copied.

**Sidebar card.** Add Reddit to a sidebar tab (Studio → Bars) for the
first five posts of a community (Studio → Plugins → Reddit → **Sidebar
card community**, or your first community), with scores where available.
A click opens the post in the panel. The card asks only while it is
shown, at most every ten minutes, through the same queue as the panel,
and forgets its posts when the sidebar hides.

**Safety.** NSFW posts are hidden by default (Studio can blur them
instead, or show them). Spoilers are covered, pictures blurred and
`>!spoiler!<` text masked, until you reveal them. Post and comment text
is Reddit's Markdown drawn as the shell's own text: raw HTML stays plain
text, and only web links (and Reddit's own `/r/` and `/u/` links) open.
Pictures come only from Reddit's image hosts.

**Requests.** One worker serves the panel, one request at a time; a new
sort or community replaces a waiting request, and leaving a post drops
its result. Every reply's `X-Ratelimit-*` headers are tracked: when the
window is used up, or after a 429, the panel waits (honouring
`Retry-After`, longer after repeated 429s, at most five minutes) and
says so instead of retrying. A page you just read is reused for two
minutes. Nothing refreshes while the panel is closed; **Refresh an open
list** (off by default) refreshes only the list on screen.

`MANDARA_REDDIT_API=http://127.0.0.1:PORT` points the plugin at a local
test server (loopback only), as the sandbox screenshots do.

## Home Assistant

Enter your Home Assistant address and save a long-lived access token
through the plugin’s keyring field. Create a token in your Home Assistant
profile. Keep the server address and token for the same instance.

Enter up to 24 favourite entity ids, separated by spaces, to choose their
order. Favourites load individually, so a large installation does not need
to download every state. Removed entities are skipped; connection and
authentication errors are reported. Leave the field empty to discover up
to 512 available entities. Sensors show real
states and units. Lights, switches, fans and input booleans offer a toggle;
scenes, scripts and buttons offer activation. An unavailable entity cannot
be controlled. Scenes and buttons can still run before they have a last-used
time; an unknown sensor value stays unknown. Locks and other unsupported
domains stay read-only.

Actions call Home Assistant’s actual services and refresh the result. They
do not create synthetic states. The panel refreshes only while open, from
every five seconds to every five minutes. Changing servers clears the old
server’s displayed values and uses a separately scoped key.

See [Commands](commands.md#connected-plugins) for the same explicit actions
over IPC. Network requests respect Mandara’s offline mode, have bounded
responses and timeouts, and never follow redirects with API keys.
