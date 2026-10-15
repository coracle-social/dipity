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

1. When the device has no identity, the app offers to get started, which makes one without explaining it, once the user has agreed to the terms of use. — `FirstRun`
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
27. The user narrows the board to some topics and content types under one filter button, or to one topic tapped on a post, and sees what everything is filed under as they read. — `ContentFilter`, in `FeedControls`, and `ItemCard`
28. The user mutes a topic from a post carrying it, and that topic leaves the board. — `ItemCard`
29. The user flags a post from its menu with every reason that applies, and the phone stops passing it on. The dialog then offers to trash the post and to mute, block and suppress its author. — `ItemCard`, `FlagPicker`
30. The user opens a post's author from its menu. — `ItemCard`
31. A post the user or a trusted contact flagged shows a warning with each reason and how many people gave it, and the user can show it anyway. — `ItemCard`
32. The user opens a flagged post's details to see who flagged it, for what, and what each person said. They can trash any flag or stop trusting the person who made it, and the phone passes the post on again once no counted flag holds it back. — `FlagDetails`

## Pairing

33. The user adds a contact by pressing a button while standing next to them, which asks everybody in range they have not named, and waits on the pairing screen until somebody opens the request. — `People`, `Pairing`
34. Somebody nearby presses that button on their own phone, and the user finds out without leaving what they were doing. Nobody is asked about a phone that merely came into range. — `PairingTray`
35. When several are asking at once, the tray says how many and opens on the oldest, the pairing screen steps between them, and answering one moves on to the next. — `PairingTray`, `Pairing`
36. The user opens a request and compares six digits against the other person's screen, so that they know the two phones are talking to each other and not to something in between; opening it is what puts the same digits on the other screen. — `Pairing`
37. The user names the person in front of them while pairing with them, because there is no profile to read, and sees what their own contacts already call that person, if anything. — `Pairing`
38. The user declines, and that person is left unnamed and not asked about again until they have left and come back. — `Pairing`
39. When the person walks away before the user answers, the request leaves the tray on its own. — `PairingTray`

## People

40. The user looks up everybody they have named, and the name each one is known by, with how many phones are in range above them, contacts or not. — `People`
41. Somebody the user never met but knows through a contact appears with that contact's name for them; a name from anybody else is not shown. — `People`
42. The user renames somebody. — `ContactDetail`
43. When the user removes somebody from their contacts, that person stops getting what the user shares with contacts and their phones meet as strangers next time. Muted, blocked and suppressed stay as they were. — `ContactDetail`
44. When the user mutes somebody, their content stops appearing without anything stopping at the wire. — `ContactDetail`
45. When the user keeps somebody's posts to themselves, they still see them, and the device never passes them on. — `ContactDetail`
46. Muting somebody hides their posts from the user, and blocking them sends them nothing. Suppressing them passes their posts to nobody and stops the user's card for them vouching on contacts' phones that trust the user. All three refuses their sessions and drops what they send. — `ContactDetail`
47. When the user stops trusting a contact's flags, what that contact flagged is drawn and passed on as if nobody had flagged it. — `ContactDetail`

## Settings

48. The user decides whose content the device stores. — `Settings`
49. The user decides who sees what they publish, and who can carry it further. — `Settings`
50. The user decides how long a thing carried for somebody else stays on the device. — `Settings`
51. The user decides whether strangers learn who they are while the app is closed. — `Settings`
52. The user writes the key down somewhere safe, locked with a password if they choose, whenever they get round to it. — `KeyBackup`, in `Settings`
53. When the user puts their key on a second phone, both phones are them. — `DeviceLogin`
54. On a new phone, the user takes the key off their other phone instead of making one. — `FirstRun`, `DeviceLogin`
55. With the phone in their pocket, the user hears that somebody nearby asked to pair or that new things reached the board, once they switch that on in Settings or when the app offers it after their second post. — `NotificationOffer`, `Settings`
56. The user decides whose pictures are blurred until tapped. — `Settings`, `Photo`
57. The user mutes a topic by typing it, and unmutes one. — `Settings`
58. A picture reaches the board once its image has, so that nothing is drawn as a hole. — `Board`
59. The user logs out, which erases the key and everything on the phone. — `Settings`

## No screen yet

Each of these is a story the transport or the docs already support and the view does not answer.

- **A notification while the app is closed.** Story 16 in the pocket. The core emits `WakeAt` and the shells own local notifications; nothing posts one.
- **Quiet times.** `policy.quiet_times` is a list of windows and needs a time picker rather than a switch. Settings leaves it alone.
- **Media beyond pictures.** A picture is the one post that attaches a file. Video, audio and a file attached to a note have no composer.
