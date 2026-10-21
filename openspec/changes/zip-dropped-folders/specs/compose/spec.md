# Spec Delta

## MODIFIED Requirements

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

## ADDED Requirements

### Requirement: No sending while a folder is being compressed
While a dropped folder is still being compressed, the system SHALL show that it is
compressing and SHALL refuse to send the message, saying why.

#### Scenario: Sending too early
- **WHEN** the user presses Send while "Compressing “Huge”…" is showing
- **THEN** the message is not sent and a notice asks them to send again in a moment
