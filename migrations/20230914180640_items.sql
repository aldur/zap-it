CREATE TABLE
  IF NOT EXISTS items (
    id INTEGER PRIMARY KEY NOT NULL,
    link TEXT NOT NULL,
    pub_date DATETIME NOT NULL
  );

CREATE INDEX pub_date_index ON items (pub_date);

CREATE UNIQUE INDEX link_index ON items (link);

