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
