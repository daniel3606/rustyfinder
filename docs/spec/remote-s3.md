# Remote: Amazon S3, S3-compatible services, Backblaze B2, Rackspace Cloud Files (P1)

The Connect panel is shared (remote-sftp.md §1).

## 1. Amazon S3 and S3-compatible

### 1.1 Connect fields
- **Access Key**, **Secret Key** (required). An error names the missing one ("Missing secret key").
- **Server:** defaults to the AWS endpoint. A custom endpoint (host[:port], http or https) enables S3-compatible services (MinIO, Wasabi, Cloudflare R2, Tigris, DigitalOcean Spaces and others). The custom server must persist in favorites and stay editable.
- **Region:** a popup with all current AWS regions (including newer ones such as Asia Pacific Malaysia, New Zealand, Taipei and Thailand; Mexico Central; Israel Tel Aviv; Canada West Calgary). For custom endpoints, a free-text region (default `us-east-1`).
- **Bucket name** (optional): when set, the connection opens inside that bucket. When empty, the root lists all buckets.
- **MFA Device** (optional, the device serial or ARN): when set, RustyFinder asks for the current one-time code at connect and gets temporary session credentials (STS GetSessionToken). The session token is used in requests and renewed by prompting again when it expires.
- Path style vs virtual-hosted style addressing: automatic. Virtual-hosted is used for AWS when the bucket name is DNS-compatible, and path style for custom endpoints *(RustyFinder choice; offer an override in Advanced)*.

### 1.2 Model and listing
- Requests are signed with **AWS Signature Version 4** (payload hash header included).
- The root lists buckets. Inside a bucket, "folders" are key prefixes delimited by `/`. A listing uses list-objects with a `/` delimiter and paginates until done. Object lists can exceed 1,000 entries.
- A zero-byte object whose key ends with `/` is a folder marker. It is shown as the folder itself, **never** as an extra file with an empty name.
- A bucket in another region: the region comes from the `x-amz-bucket-region` response header, and the request is retried against the right regional endpoint.
- Modification time = the object's Last-Modified.

### 1.3 Operations
- **New Bucket** (File menu on the root): name plus region. Success and failure are logged truthfully ("Created bucket …" or an error).
- **New Folder:** creates a zero-byte `prefix/` marker.
- **Upload:** single PUT for files ≤ 100 MB *(RustyFinder choice)*. Larger files use **multipart upload** (part size 16 MB, or larger so a file stays under 10,000 parts; parts are uploaded in parallel within the transfer limit). A failed or stopped multipart upload is **aborted** so no orphaned parts remain.
- **Content-Type** is set from the file extension (a system UTI to MIME mapping) for every upload, including uploads coming from **another remote location** and **multipart uploads of files over 5 GB**. Only unknown types fall back to `application/octet-stream`.
- **Storage Class** (upload option and Info field): STANDARD (default), REDUCED_REDUNDANCY, STANDARD_IA, ONEZONE_IA, INTELLIGENT_TIERING, GLACIER, GLACIER_IR, DEEP_ARCHIVE. Downloading a GLACIER or DEEP_ARCHIVE object that hasn't been restored gives a clear error.
- **Rename/move** of an object = server-side copy (copy-source header) then delete. For folders, every object under the prefix is copied and deleted, with progress. Renaming folders must work.
- **Delete** of a folder deletes every key under the prefix (batch delete where available) and the marker.
- **Resume** isn't offered for uploads. Downloads may resume with HTTP Range *(RustyFinder choice)*.
- Permissions: the Info section shows the owner. POSIX permission editing is not available.
- "Copy URL" copies `https://<bucket>.s3.<region>.amazonaws.com/<key>` (or the custom endpoint form) without credentials.

## 2. Backblaze B2 (native API)

- Fields: **Key ID** (account or application key ID) and **Application Key**. Optional bucket.
- Authorizes through the B2 authorize-account call, then uses the returned API URL and download URL. The auth token is refreshed automatically on 401 "expired".
- Listing by file name with a `/` delimiter (folders are prefixes). Hidden or old file versions are not shown.
- Upload: get an upload URL, then upload with a SHA-1 checksum. Large files (> 100 MB *(RustyFinder choice)*) use the start, upload-part, finish large-file flow. A stopped large upload is cancelled on the server.
- Delete removes the file version(s) shown. Rename = server-side copy then delete.
- Wrong credentials give "Authorization failed", never a crash, including on repeated attempts.
- A 500/503 reply is retried with back-off (file-ops §5.5).

## 3. Rackspace Cloud Files (OpenStack Swift)
- Fields: username, API key, region. Containers are the top level and pseudo-folders come from `/` delimiters. Upload, download, delete, and rename (copy plus delete). Large objects over 5 GB use segmented upload *(RustyFinder choice; P2 depth)*.

## 4. Acceptance tests

Fixture: MinIO (S3-compatible) on localhost. B2 tests use a mock server.

- **S3-01** Given MinIO at `localhost:9000` with path style, when the user connects with keys and no bucket, then buckets are listed.
- **S3-02** Given a bucket with 2,500 objects in one prefix, then all 2,500 are listed.
- **S3-03** Given folder marker `docs/` and object `docs/a.txt`, then the listing of the bucket root shows a folder `docs`, and inside it only `a.txt`. No empty-named item appears.
- **S3-04** Given a 300 MB upload, then the object exists with the correct ETag-based integrity, and no incomplete multipart uploads remain afterwards.
- **S3-05** Given a stopped 300 MB upload, then the server has no in-progress multipart upload for that key.
- **S3-06** Given an upload of `photo.jpg` from an SFTP location to S3, then its Content-Type is `image/jpeg`.
- **S3-07** Given a rename of folder `old/` (3 objects) to `new/`, then the 3 objects exist under `new/` and none under `old/`.
- **S3-08** Given a bucket in region eu-west-1 opened while the connection region is us-east-1, then its contents still list.
- **S3-09** Given an MFA device is set, when connecting, then a code prompt appears and requests carry a session token.
- **S3-10** Given B2 credentials that are wrong three times in a row, then each attempt shows an auth error and the app doesn't crash.
