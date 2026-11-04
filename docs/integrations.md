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

Save the key in the plugin’s masked keyring field. Keys live in your
desktop’s Secret Service keyring, scoped to the provider and server,
and stay out of Mandara’s configuration and request URLs. Removing a key
removes only that provider’s key for that server. A locked keyring is
reported without opening an unlock prompt just because a panel opened.

Choose source and target languages from your provider’s current list.
**Auto-detect** leaves the source to the provider. Type or explicitly
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

Click a headline to open its web address in your browser. It is marked
read after the browser opens successfully. A feed keeps its latest 100
entries, and read flags are kept for those entries. Subscriptions and
read history stay on this computer.

## Home Assistant

Enter your Home Assistant address and save a long-lived access token
through the plugin’s keyring field. Create a token in your Home Assistant
profile. Keep the server address and token for the same instance.

Enter up to 24 favourite entity ids, separated by spaces, to choose their
order. Leave the field empty to show available entities. Sensors show real
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
