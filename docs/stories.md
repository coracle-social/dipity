# Dip — user stories

What a person does with the app, in the order they meet it. [`ui.md`](./ui.md) covers how it looks; this is what has to be reachable.

Each story names the screen that answers it. A story with no screen is a gap, and the ones that have none yet are listed at the foot.

## What the stories rest on

Four facts about the transport decide most of the interface, and every story below inherits them.

**Nothing is guaranteed to arrive.** Content moves over Bluetooth, hop by hop, and stops at two. So the app never promises delivery, never shows a sent-and-read state, and never offers a private channel. A reply is said to the user's own neighbours rather than addressed to the author, so a comment can reach people who never got the thing it answers. A boost names what it passes on rather than carrying it, and a comment names what it answers, so either can outrun its subject. The board leaves both out until the subject arrives. Nothing here is addressed to anybody, and the word for passing something on is **boost**.

**Passing something on takes permission.** An event travels a second hop only from a device holding the author's signature naming it, so a thing carried in from further out can be read and not forwarded. A boost or a reaction is the user's own event and travels either way; what it names may not.

**Arrival time is not authorship time.** An event carries when it was written and the device records when it turned up. The feed is ordered by arrival, because that is the order the user lived through, and the other ordering is one tap away.

**People are named by their neighbours.** Nobody publishes a profile. A name is what one person calls another, carried on a card that travels with the named person's events, so a name always arrives with a claimant attached — yours, or the person who handed it over.

## First run

1. The device has no identity, so the app offers to make one, and says the key stays on the phone. — `FirstRun`
2. The user has a key from another device and pastes it in. — `FirstRun`
3. The identity exists, so the app opens the store and starts the radio without asking anything. — `App`

## The board

4. The user opens the app and sees what has reached the device, most recently arrived first. — `Board`
5. The user wants the same list in the order it was written, so a two-way switch re-sorts it. — `FeedControls`
6. The user is only interested in some kinds of thing, so a filter narrows the list to the ones they pick. — `FeedControls`
7. The user reads an item and can tell who wrote it, by the name they know them by. — `Byline`
8. The name came from somebody else, so the item says whose name it is and de-emphasises the claim. — `Byline`
9. The user can see when a thing reached the device and who handed it over. — `ItemCard`
10. The user can see that an item is on its way out of the store, so nothing disappears unexplained. — `ItemCard`
11. The user writes something and it goes out to whoever comes into range next. — `Composer`
12. The user passes something on, saying something about it or sending it as it is, and one dialog offers both. — `Composer`
13. The user taps a boost or a comment and lands on the thing it is about, wherever on the card they tapped. — `ItemCard`
14. The user reacts, picking from every emoji there is rather than from a shortlist. — `EmojiPicker`
15. The user sees what other people made of something, and who each reaction came from. — `Reactions`
16. The user opens one thing on its own and reads what people are saying about it. — `ItemDetail`
17. The user reads a comment on a comment and can climb to whatever that one answered. — `ItemDetail`, `Quoted`
18. The user goes back, with the phone's own button or the one on the screen, and lands on what they came from as far down it as they had read. — `nav`
19. The user deletes something they published, and the request travels the same way the thing did. — `ItemCard`
20. The user drops somebody else's thing off their own device, which tells nobody and asks nothing. — `ItemCard`
21. The user bookmarks something, so the retention sweep leaves it alone however long it stops circulating. — `ItemCard`
22. The user looks at everything they bookmarked, whatever the board's filter is set to. — `Bookmarks`
23. The user writes a poll, a calendar event or an article rather than a note, and the form changes to suit. — `Composer`
24. The user opens a thing and sees how far it has travelled: how many people handed it to this device, and how many this device has handed it to. — `ItemDetail`

## Pairing

25. Somebody comes into range and asks to pair, and the user finds out without leaving what they were doing. — `PairingTray`
26. Several are asking at once, so the tray says how many and opens on the oldest. — `PairingTray`
27. The user opens a request and compares five shapes against the other person's screen, so they know the two phones are talking to each other and not to something in between. — `Pairing`
28. The user names the person in front of them while pairing with them, because there is no profile to read. — `Pairing`
29. The name lands on whoever the link turns out to be, which the core says after the gate has passed. — `pairing.ts`
30. The user declines, and is not asked about that person again for a while. — `Pairing`
31. The person walks away before the user answers, so the request leaves the tray on its own. — `PairingTray`

## People

32. The user looks up everybody the device knows, and the name each one is known by. — `People`
33. Somebody the user never met is known through a neighbour, so they appear with the neighbour's name for them. — `People`
34. The user renames somebody. — `ContactDetail`
35. The user trusts somebody, which is what lets that person carry their content onward. — `ContactDetail`
36. The user mutes somebody, so their content stops appearing without anything stopping at the wire. — `ContactDetail`
37. The user blocks somebody, so the device refuses their sessions and drops what they send. — `ContactDetail`

## Settings

38. The user decides whose content the device stores, carries onward, and may share forward. — `Settings`
39. The user decides who may see what they publish. — `Settings`
40. The user decides how long a thing carried for somebody else stays on the device. — `Settings`
41. The user decides how long the device keeps answering strangers after the app goes into the pocket, and how many strangers it will name itself to. — `Settings`
42. The user writes the key down somewhere safe. — `Settings`
43. The user puts their key on a second phone, so both phones are them. — `DeviceLogin`
44. The user takes the key off their other phone, and this one stops being the identity it made at first run. — `DeviceLogin`

## No screen yet

Each of these is a story the transport or the docs already support and the view does not answer.

- **A notification while the app is closed.** Story 16 in the pocket. The core emits `WakeAt` and the shells own local notifications; nothing posts one.
- **Discoverable times.** `policy.discoverable_times` is a list of windows and wants a time picker rather than a switch. Settings edits the other three discoverability preferences and leaves it alone.
- **Per-contact gossip.** [`policy.md`](./policy.md#accept-and-gossip) expresses gossip as a tier over the whole trust graph, so "do not pass this person's things on" is only sayable as a block. `ContactDetail` offers trust, mute and block.
- **Media.** The first version carries text. `imeta`, previews and blob transfer are built below the bridge and nothing above it attaches a file.
