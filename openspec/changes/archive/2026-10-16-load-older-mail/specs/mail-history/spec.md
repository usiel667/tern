# Spec Delta

## Purpose

Defines how much of each mailbox Tern keeps on this computer and how older mail is shown and
downloaded when the user scrolls back through a mailbox.

## ADDED Requirements

### Requirement: Recent mail is downloaded first
On first sync of a mailbox the system SHALL download the newest messages up to a fixed limit
(1,000) rather than the whole mailbox, so a new account is usable quickly.

#### Scenario: Large mailbox on first launch
- **WHEN** an account with a 50,000-message inbox is added
- **THEN** the newest 1,000 inbox messages are downloaded and shown, and older ones are not downloaded yet

### Requirement: Scrolling shows more of the cached mail
When the user scrolls to the bottom of the message list and more cached messages match the
current view than are shown, the system SHALL show more of them without contacting the
server.

#### Scenario: Cached rows beyond the first page
- **WHEN** a mailbox has more cached messages than the list currently shows and the user scrolls to the bottom
- **THEN** further older messages appear below without a download

### Requirement: Scrolling past the cache downloads older mail
When the user scrolls to the bottom of a mailbox's list and every cached message is already
shown, the system SHALL download the next batch of older messages for that mailbox from the
server and add them to the list.

#### Scenario: Reaching the end of the cache
- **WHEN** the user scrolls to the bottom of the inbox and all cached inbox messages are shown
- **THEN** the next batch of older inbox messages is downloaded and appears at the bottom of the list

#### Scenario: Repeated scrolling while loading
- **WHEN** the user scrolls to the bottom again while an older batch is still downloading
- **THEN** no duplicate download is started

### Requirement: Older mail is kept in sync
Older messages the system has downloaded SHALL be treated like other cached messages: changes
to their flags or read state and their deletion on the server SHALL be reflected locally, and
they SHALL be included in local search.

#### Scenario: Older message read on another device
- **WHEN** an older message that was downloaded by scrolling is marked read on another device
- **THEN** it shows as read in Tern after the next sync

#### Scenario: Searching older mail
- **WHEN** the user searches for a word that appears only in a downloaded older message
- **THEN** that message is in the results

### Requirement: Loading stops at the start of the mailbox
When the server reports no messages older than those cached, the system SHALL stop requesting
older mail for that mailbox for the rest of the session.

#### Scenario: Whole mailbox downloaded
- **WHEN** the user scrolls to the bottom after every message in the mailbox has been downloaded
- **THEN** no further request for older mail is made for that mailbox until Tern restarts

### Requirement: Unified views load each included mailbox
When the view is a unified view of one mailbox type across accounts (such as All Inboxes),
scrolling past the cache SHALL download older mail for each mailbox in that view. Views not
tied to mailboxes (Flagged, Unread, search results) SHALL NOT trigger downloads.

#### Scenario: All Inboxes with two accounts
- **WHEN** the user scrolls to the bottom of All Inboxes with two accounts and all cached inbox mail is shown
- **THEN** older mail is downloaded for both accounts' inboxes

#### Scenario: Flagged view
- **WHEN** the user scrolls to the bottom of the Flagged view
- **THEN** no older mail is downloaded
