# Dipity — user stories

What a person does with the app, in the order they meet it. [`ui.md`](./ui.md) covers how it looks; this is what has to be reachable.

Each story names the screen that answers it. A story with no screen is a gap, and the ones that have none yet are listed at the foot.

## What the stories rest on

Four facts about the transport decide most of the interface, and every story below inherits them.

**Nothing is guaranteed to arrive.** Content moves over Bluetooth, hop by hop, and stops at two. So the app never promises delivery, never shows a sent-and-read state, and never offers a private channel. A comment can reach people who never got the thing it answers, because a reply is said to the user's own neighbours rather than addressed to the author. A boost or a comment can outrun its subject, because a boost names what it passes on rather than carrying it and a comment names what it answers. The board leaves both out until the subject arrives. Nothing here is addressed to anybody, and the word for passing something on is **boost**.

**Passing something on takes permission.** An event travels a second hop only from a device holding the author's signature naming it. A thing carried in from further out can be read and not forwarded. A boost or a reaction is the user's own event and travels either way; what it names may not.

**Arrival time is not authorship time.** An event carries when it was written and the device records when it turned up. The feed is ordered by arrival, because that is the order the user lived through, and the other ordering is one tap away.

**People are named by their neighbours.** Nobody publishes a profile. A name is what one person calls another, carried on a card that travels with the named person's events. A name always arrives with a claimant attached — yours, or the person who handed it over.

## First run

1. When the device has no identity, the app offers to get started, which makes one without explaining it. — `FirstRun`
2. The user has a key from another device and logs in with it. — `FirstRun`
3. When the identity exists, the app opens the store and starts the radio without asking anything. — `App`

## The board

4. The user opens the app and sees what has reached the device, most recently arrived first. — `Board`
5. When the user wants the same list in the order it was written, a two-way switch re-sorts it. — `FeedControls`
6. When the user is only interested in some kinds of thing, a filter narrows the list to the ones they pick. — `FeedControls`
7. The user reads an item and can tell who wrote it, by the name they know them by. — `Byline`
8. When the name came from somebody else, the item says whose name it is and de-emphasises the claim. — `Byline`
9. The user can see when a thing reached the device and who handed it over. — `ItemCard`
10. Nothing disappears unexplained, because the user can see that an item is on its way out of the store. — `ItemCard`
11. The user writes something and it goes out to whoever comes into range next. — `Composer`
12. The user passes something on, saying something about it or sending it as it is, and one dialog offers both. — `Composer`
13. The user taps a boost or a comment and lands on the thing it is about, wherever on the card they tapped. — `ItemCard`
14. The user reacts, picking from every emoji there is rather than from a shortlist. — `EmojiPicker`
15. The user sees what other people made of something, and who each reaction came from. — `Reactions`
16. The user opens one thing on its own and reads what people are saying about it. — `ItemDetail`
17. The user reads a comment on a comment and can climb to whatever that one answered. — `ItemDetail`, `Quoted`
18. The user goes back, with the phone's own button or the one on the screen, and lands on what they came from as far down it as they had read. — `nav`
19. The user throws something out, and it waits in the trash for a week, where they can put it back. Throwing out what they published retracts it at once, and putting it back restores it the same way, by a request that travels the way the thing did, and somebody else's retraction puts their thing in the trash the same way. — `ItemCard`, `Bookmarks` under Trash
20. The trash empties, by hand or after the week, dropping everything in it off this phone, which tells nobody. — `Bookmarks` under Trash
21. When the user bookmarks something, the retention sweep leaves it alone however long it stops circulating. — `ItemCard`
22. The user looks at everything they bookmarked, whatever the board's filter is set to. — `Bookmarks`, under Saved
23. The user writes a poll, a calendar event, an article or a picture rather than a note, and the form changes to suit. — `Composer`
24. The user opens a thing and sees how far it has travelled: how many people handed it to this device, and how many this device has handed it to. — `ItemDetail`
25. The user looks for something they half remember, and the board narrows as they type to what says it and to what was written by somebody called it; their people and their bookmarks search the same way. — `SearchBox`, in `FeedControls`, `People` and `Bookmarks`
26. The user files what they write under a topic they type, or under none. — `Composer`
27. The user narrows the board to one topic, picked from those on it or tapped on a post, and sees what everything is filed under as they read. — `TopicFilter`, in `FeedControls`, and `ItemCard`
28. The user mutes a topic from a post carrying it, and that topic leaves the board. — `ItemCard`

## Pairing

29. Somebody the user has not named comes into range and asks to pair, whether or not the gate let them through, and the user finds out without leaving what they were doing. — `PairingTray`
30. When several are asking at once, the tray says how many and opens on the oldest, the pairing screen steps between them, and answering one moves on to the next. — `PairingTray`, `Pairing`
31. The user opens a request and compares five shapes against the other person's screen, so that they know the two phones are talking to each other and not to something in between. — `Pairing`
32. The user names the person in front of them while pairing with them, because there is no profile to read, and sees what their own contacts already call that person, if anything. — `Pairing`
33. The name lands on whoever the link turns out to be, which the core says after the gate has passed. — `pairing.ts`
34. The user declines a stranger the gate is holding, and is not asked about that person again for a while; one already through is just left unnamed until they next meet. — `Pairing`
35. When the person walks away before the user answers, the request leaves the tray on its own. — `PairingTray`

## People

36. The user looks up everybody the device knows, and the name each one is known by. — `People`
37. Somebody the user never met but knows through a contact appears with that contact's name for them; a name from anybody else is not shown. — `People`
38. The user renames somebody. — `ContactDetail`
39. When the user forgets somebody, they stop being a contact. — `ContactDetail`
40. When the user mutes somebody, their content stops appearing without anything stopping at the wire. — `ContactDetail`
41. When the user blocks somebody, the device refuses their sessions and drops what they send. — `ContactDetail`

## Settings

42. The user decides whose content the device stores. — `Settings`
43. The user decides who sees what they publish, and who can carry it further. — `Settings`
44. The user decides how long a thing carried for somebody else stays on the device. — `Settings`
45. The user decides whether strangers learn who they are while the app is closed. — `Settings`
46. The user writes the key down somewhere safe, locked with a password if they choose, whenever they get round to it. — `KeyBackup`, in `Settings`
47. When the user puts their key on a second phone, both phones are them. — `DeviceLogin`
48. On a new phone, the user takes the key off their other phone instead of making one. — `FirstRun`, `DeviceLogin`
49. With the phone in their pocket, the user hears that somebody nearby wants to pair or that new things reached the board, once they switch that on in Settings or when the app offers it after their second post. — `NotificationOffer`, `Settings`
50. The user decides whose pictures are blurred until tapped. — `Settings`, `Photo`
51. The user mutes a topic by typing it, and unmutes one. — `Settings`
52. A picture reaches the board once its image has, so that nothing is drawn as a hole. — `Board`
53. The user logs out, which erases the key and everything on the phone. — `Settings`

## No screen yet

Each of these is a story the transport or the docs already support and the view does not answer.

- **A notification while the app is closed.** Story 16 in the pocket. The core emits `WakeAt` and the shells own local notifications; nothing posts one.
- **Quiet times.** `policy.quiet_times` is a list of windows and needs a time picker rather than a switch. Settings leaves it alone.
- **Per-contact relaying.** "Do not pass this person's things on" is only sayable as a block, because [relaying](./policy.md#relaying) hands every contact everything this device may forward. `ContactDetail` offers mute and block.
- **Media beyond pictures.** A picture is the one post that attaches a file. Video, audio and a file attached to a note have no composer.
