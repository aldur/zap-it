# Zap-It-Later ⚡

Lightweight web-app to add inbound links to an RSS feed.

Rationale: I enjoy [Miniflux](https://miniflux.app). Zap-It-Later, together with
an iOS/macOS Shortcut, allows me to add web links to an RSS feed; Miniflux will
later pull items from the feed, fetch them (relying on its "Fetch original
content" option) and add to my timeline. This quickly allow me to archive pages
and/or read them later.

## API

### `feed.xml`

Point your RSS header to `/feed.xml`.

```bash
curl http://localhost:3000/feed.xml
```

### Add a new `link`

Issue an HTTP `POST` to `/add`, providing a `json` object including the `link`
and `title` keys:

```bash
curl --json '{"link":  "https://github.com/aldur/zap-it-later", "title": "Zap-It-Later ⚡"}' http://localhost:3000/add
```

## Build

If using `nix`:

```bash
nix build
nix run
```

### Docker image

Through `nix`:

```bash
nix build .#dockerImage && ./result | docker load
```

## Run

### Configuration

Configure the following environmental variables:

- `DATABASE_URL`: points to the `sqlite` DB path
- `DOMAIN`: fully qualified domain (including `https://`), required by the
  [RSSv2 
  specification](https://www.rssboard.org/rss-draft-1#element-channel-link).
- `LISTEN_IFACE`: listen interface
- `LISTEN_PORT`: listen port

### Access control

This web-app keeps things as simple as possible, and expects access control
mechanisms to be implemented _above_ it.

For instance, `nginx` (or another reverse proxy) let us configure
[Authelia](https://www.authelia.com) or HTTP basic auth to protect the exposed
routes.

Alternatively, [Tailscale](https://tailscale.com/kb/1223/tailscale-funnel/)'s
funnel enables exposing specific routes (and even take care of HTTPS!) and
taking care of access control.

## `nginx` reverse proxy and HTTP basic authentication

In the following example, we set up `nginx` as a reverse proxy for a Docker
container running `zap_it_later`, protect it through HTTP basic auth and _only_
expose the `/add` route -- since the RSS reader will fetch `feed.xml` directly
through Docker's internal network.

```nginx
# ...
    auth_basic           "Zap";
    auth_basic_user_file /etc/nginx/conf.d/zap.htpasswd;

    # NOTE: We don't need to expose `feed.xml`, since `miniflux` will route
    # through the Docker network directly.
    location = /add {
        set $upstream_app zap_it_later;
        set $upstream_port 3000;
        set $upstream_proto http;
        proxy_pass $upstream_proto://$upstream_app:$upstream_port;
    }
```
