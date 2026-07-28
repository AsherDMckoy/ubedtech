# Running the UB education platform from a release package

The package contains everything except a database:

| File | What it is |
|---|---|
| `ubedtech` / `ubedtech.exe` | the server (schema migrations are built in) |
| `dist/` | frontend assets the server loads at startup |
| `seed.sql` | optional demo accounts and data |
| `RUN.md` | this file |

## 1. Start PostgreSQL (Docker)

Install Docker (Docker Desktop on Windows, `docker` on Linux), then:

```sh
docker pull postgres:17
docker run --name ubedtech-pg \
  -e POSTGRES_PASSWORD=ubedtech \
  -e POSTGRES_DB=ubedtechdb \
  -v ubedtech-pgdata:/var/lib/postgresql/data \
  -p 5432:5432 -d postgres:17
```

On Windows PowerShell use backticks (`` ` ``) instead of `\` for line
continuations, or put the command on one line. The named volume
`ubedtech-pgdata` keeps the data across container restarts. After a
reboot, `docker start ubedtech-pg` brings the same database back.

## 2. Start the server

The server applies its schema migrations automatically on startup, so a
fresh empty database is all it needs.

Linux:

```sh
DATABASE_URL=postgresql://postgres:ubedtech@localhost:5432/ubedtechdb \
APP_FRONTEND_DIST=./dist \
./ubedtech
```

Windows (PowerShell, from the unzipped folder):

```powershell
$env:DATABASE_URL = "postgresql://postgres:ubedtech@localhost:5432/ubedtechdb"
$env:APP_FRONTEND_DIST = ".\dist"
.\ubedtech.exe
```

Then open <http://localhost:8080/ui/login>.

## 3. Load data

Load your own SQL the same way, or use the bundled demo dataset
(run this AFTER the server has started once, so the schema exists):

```sh
docker exec -i ubedtech-pg psql -U postgres -d ubedtechdb < seed.sql
```

Demo accounts (all passwords `ub-demo-password`): `demo.student`,
`demo.held`, `demo.instructor`, `demo.registrar`, `demo.admin`,
`demo.platform`.

## Configuration notes

- Binds `0.0.0.0:8080` by default — override with `APP_BIND_ADDR`.
- Generated documents are written to `./var/documents` — override with
  `APP_DOCUMENT_STORAGE_PATH`.
- `APP_ENV=production` turns on `Secure` cookies, which require HTTPS in
  front of the server. Leave it unset for local, plain-HTTP use.
