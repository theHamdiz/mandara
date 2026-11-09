# Connected plugins

Translator, Phone Connect, Feeds and Home Assistant are bundled with
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

A reader for RSS 0.9x, RSS 1.0 (RDF), RSS 2.0, Atom and JSON Feed. The bar
item shows how many articles are unread (as a badge, a number or a dot:
Studio → Plugins → Feeds → Settings → *Unread count*). The panel has a
sidebar with **All articles**, **Saved articles**, your folders and feeds,
each with its unread count, and one timeline of article cards: the feed's
icon and colour, how long ago it was published, an unread dot, the title,
a short excerpt and the feed's thumbnail. Search finds words in titles,
excerpts, authors and feed names; **All | Unread** filters the list. The
toolbar refreshes every feed, marks everything in the selected list read
and opens the settings.

**Reading.** Click a card (or press Enter) to open it in the reader: a
clean copy of the article drawn with Mandara's own text, without scripts,
frames, forms, embedded media or tracking pixels, and with only web links.
Its toolbar saves the article for later, marks it read or unread, copies
the link and opens it in your browser. *Open articles in* can send
articles straight to the browser instead. Opening an article marks it
read (*Mark read when opened*). Keys: ↑ ↓ or J K move, Enter opens,
Esc goes back, O opens the browser, M toggles read, S saves, C copies the
link, R refreshes and / searches.

**Sidebar card.** Add Feeds to the sidebar (Studio → Bars → Sidebar) for a
card with the newest unread articles, each with its feed's monogram and
colour, how long ago it came out and an unread dot. *Card folder* limits it
to one folder. Clicking an article opens it in the panel's reader; the
header opens the panel. The card never downloads anything: it shows what
the panel last fetched, and lets go of its rows when the sidebar hides.

**Pictures.** *Pictures* chooses **None**, **Icons and thumbnails** (the
default: the feed's icon and the thumbnails the feed declares) or **Also
inside articles**, which loads pictures inside the reader from their
websites. Only pictures on screen load, decoded at the size they are
drawn, and a small disk cache keeps them for next time.

**Adding feeds.** Studio → Plugins → Feeds → Settings → *Your feeds*: type
a site or feed address and **Add**. For a site, Mandara reads the feed its
page announces (or tries the usual addresses such as `/feed` and
`/atom.xml`); a site with several feeds lets you pick. Each feed can be
renamed, put in a folder, given a colour, moved up or down and removed.
*Starter feeds* lists popular feeds by topic (technology, Linux and open
source, programming, science, world news, Arabic news, design and fun),
each checked to download and read correctly. *Import and export* reads
and writes OPML files from and for other feed readers. In the launcher,
typing a web address offers **Subscribe in Feeds**, and `mandara msg feeds
subscribe <address>` does the same (both turn Feeds on). Up to 64 feeds.

**Refreshing.** Nothing downloads while nothing shows the feeds. When you
open the panel, feeds older than *Refresh while open* (five minutes by
default) refresh, three at a time; the Refresh button fetches all of them.
*Refresh in the background* (off by default) keeps the unread count
current with one coalesced refresh every *Background interval* (an hour
by default, never more often than every 15 minutes), and *Notify about new
articles* (off by default) sends one notification for what is new, at
most every 30 minutes and never while the panel is open. Requests are
compressed, reuse the server's ETag and modification date, follow up to
five redirects (only to web addresses, never from https down to http, and
a permanent move is remembered), and respect a busy server's Retry-After.
A feed that fails waits longer before each new try, from five minutes up
to six hours, and the panel says when it will try again; the last
articles stay readable meanwhile.

**History.** Each feed keeps its newest *Articles per feed* (50 by
default) plus every saved article. Articles that leave the feed stay for
*Keep old articles for* (14 days by default); saved ones stay until you
remove them. Read and saved flags, the articles and cleaned-up article
text (up to 2 MiB per feed) stay on this computer, in
`~/.local/state/mandara/feeds/`, with owner-only permissions. The panel
loads them when it opens and lets go of them, and of every decoded
picture, shortly after it closes; the bar keeps only the counts.

Feed lists from earlier versions (space-separated addresses) keep working
and move to the new list the first time you change it in Studio.

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
