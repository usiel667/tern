# compose Specification

## Purpose
Defines how new messages are started and pre-filled, including from `mailto:` links coming
from inside messages or from other applications.

## Requirements

### Requirement: mailto links open a pre-filled New Message
The system SHALL open a New Message window filled from a `mailto:` URI, whether the link is
clicked in a message or passed to `tern` by another application or the command line.

#### Scenario: Link from another application
- **WHEN** another application runs `tern mailto:ann@example.com?subject=Hi`
- **THEN** a New Message window opens with To "ann@example.com" and Subject "Hi"

#### Scenario: Tern already running
- **WHEN** Tern is running and `tern mailto:ann@example.com` is run again
- **THEN** the running Tern opens the New Message window and no second instance starts

#### Scenario: Not a mailto link
- **WHEN** `tern` is given an argument that isn't a `mailto:` URI
- **THEN** Tern shows a notice that it can only open `mailto:` links and opens nothing else

### Requirement: mailto parsing follows RFC 6068
The system SHALL read the `to`, `cc`, `bcc`, `subject`, `body` and `in-reply-to` fields of a
`mailto:` URI, with case-insensitive field names, percent-decoding, repeated recipient fields
adding to each other, and `+` kept as a literal character.

#### Scenario: Plus addressing
- **WHEN** the URI is `mailto:me+tag@example.com`
- **THEN** the recipient is "me+tag@example.com"

#### Scenario: Multiple recipients and a multi-line body
- **WHEN** the URI is `mailto:a@x.org?to=b@x.org&CC=c@x.org&body=line%201%0D%0Aline%202`
- **THEN** To is a@x.org and b@x.org, Cc is c@x.org, and the body is two lines

### Requirement: mailto links cannot attach files or set other headers
The system SHALL ignore every `mailto:` field other than the ones it reads, including
`attach` and `attachment`.

#### Scenario: Attachment field in a link
- **WHEN** a link is `mailto:x@y.org?attach=/home/me/.ssh/id_rsa`
- **THEN** the New Message window has no attachments

### Requirement: Recipient fields suggest known addresses
While the user types in the To, Cc or Bcc field, the system SHALL suggest up to 8 addresses
from cached mail whose email or any word of whose name starts with the text typed after the
last comma, excluding the user's own addresses and addresses already in that field.

#### Scenario: Suggesting by name
- **WHEN** the user has received mail from "Maya Chen <maya@example.com>" and types "ch" in To
- **THEN** "Maya Chen <maya@example.com>" is suggested

#### Scenario: Picking a suggestion
- **WHEN** suggestions are showing and the user presses Return or Tab, or clicks one
- **THEN** the typed text is replaced by the chosen address followed by ", " and suggestions close

#### Scenario: Keys keep their usual meaning without suggestions
- **WHEN** no suggestions are showing and the user presses Escape or Ctrl+Return
- **THEN** the compose window behaves as before (asks to discard, or sends)

### Requirement: People you write to rank first
The system SHALL rank addresses the user has sent mail to above addresses that only sent
mail to the user, then by how often and how recently they were seen.

#### Scenario: Colleague above newsletter
- **WHEN** the user has written once to "nora@work.com", has received ten newsletters from "news@shop.com", and types "n"
- **THEN** "nora@work.com" is suggested before "news@shop.com"

### Requirement: Junk and machine senders are not suggested
The system SHALL NOT learn addresses from messages in Junk, Trash or Drafts, nor
automated addresses such as noreply or mailer-daemon.

#### Scenario: Spam in Junk
- **WHEN** a message from "deals@spam.example" arrives in Junk
- **THEN** typing "deals" suggests nothing for that address

### Requirement: Existing mail is used immediately after upgrading
The system SHALL learn addresses from mail that was cached before this feature existed.

#### Scenario: Upgrade with cached mail
- **WHEN** Tern starts on a cache created by an earlier version
- **THEN** addresses from the already-cached mail are suggested without waiting for a sync

### Requirement: Files dropped on a New Message are attached
The system SHALL attach every regular file dropped anywhere on a New Message window,
including the message body, reading them without blocking the window. A dropped folder
SHALL be attached as a zip archive of its contents named after the folder, leaving out
hidden entries and symlinks.

#### Scenario: Dropping files from a file manager
- **WHEN** the user drags two files from the file manager onto the message body and drops them
- **THEN** both appear as attachments and the body text is unchanged

#### Scenario: Dropping a folder
- **WHEN** the user drops a folder named "Project" containing files, a subfolder and a hidden ".git" folder
- **THEN** "Project.zip" is attached, containing the files and subfolder but not ".git"

#### Scenario: Dropping a very large folder
- **WHEN** the user drops a folder whose files add up to more than 200 MB
- **THEN** nothing is attached for it and a notice says the folder is too large

#### Scenario: Dropping a web link
- **WHEN** the user drops a link dragged from a web browser
- **THEN** the link is inserted into the body as text and nothing is downloaded or attached

### Requirement: Drag feedback
While files are dragged over a New Message window the system SHALL show that dropping will
attach them.

#### Scenario: Hovering with files
- **WHEN** files are dragged over the window
- **THEN** the window shows an outline and "Drop to attach" until the drag leaves or drops

### Requirement: Attachment sizes are visible
The system SHALL show each attachment's size and warn when the total exceeds 25 MB.

#### Scenario: Large attachments
- **WHEN** the attachments add up to more than 25 MB
- **THEN** a notice warns that many mail servers reject messages that large

### Requirement: No sending while a folder is being compressed
While a dropped folder is still being compressed, the system SHALL show that it is
compressing and SHALL refuse to send the message, saying why.

#### Scenario: Sending too early
- **WHEN** the user presses Send while "Compressing “Huge”…" is showing
- **THEN** the message is not sent and a notice asks them to send again in a moment

### Requirement: Messages being written are saved as drafts
While a New Message window has content, the system SHALL save it as a draft a few seconds
after typing stops and at least every 30 seconds while typing continues, and when the user
presses Ctrl+S. Each save SHALL be kept on this computer immediately and uploaded to the
account's Drafts mailbox, replacing the previous server copy.

#### Scenario: Autosave while writing
- **WHEN** the user types a message and pauses
- **THEN** within a few seconds the message appears in the account's Drafts mailbox, once

#### Scenario: Offline
- **WHEN** the user writes a message while offline
- **THEN** the draft is kept on this computer and uploaded to Drafts when the account is reachable again

#### Scenario: Bcc in drafts
- **WHEN** a draft with a Bcc recipient is reopened
- **THEN** the Bcc recipient is still there

### Requirement: Closing a message offers to keep it
When the user closes a New Message window that has content, the system SHALL ask whether to
save it as a draft: Save keeps the draft, Don't Save removes any saved draft, Cancel returns
to the message. A window without content SHALL close without asking.

#### Scenario: Don't Save after autosave
- **WHEN** a message was autosaved and the user closes it and chooses Don't Save
- **THEN** the draft is removed from Drafts

### Requirement: Sending removes the draft
When a message that was saved as a draft is sent, the system SHALL remove its draft.

#### Scenario: Send after autosave
- **WHEN** the user sends a message that had been autosaved
- **THEN** the message is sent and no copy is left in Drafts

### Requirement: Drafts can be edited
The system SHALL open a message from a Drafts mailbox for editing (Edit Draft in the viewer,
or double-click/Return in the list), with its recipients including Bcc, subject, text and
attachments, and later saves SHALL replace that server copy.

#### Scenario: Draft written elsewhere
- **WHEN** the user opens a draft that was written on their phone
- **THEN** it opens in a New Message window, and saving it replaces the phone's copy instead of adding a second one
