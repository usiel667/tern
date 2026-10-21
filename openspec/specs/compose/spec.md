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
