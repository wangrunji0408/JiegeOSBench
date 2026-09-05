set -ex
apk add --no-cache build-base pcre2-dev linux-headers wget >/dev/null
cd /tmp
wget -q https://nginx.org/download/nginx-1.26.2.tar.gz
tar xzf nginx-1.26.2.tar.gz
cd nginx-1.26.2
./configure \
  --prefix=/nginx \
  --with-cc-opt="-static -O2" \
  --with-ld-opt="-static" \
  --without-http_gzip_module \
  --without-http_rewrite_module \
  --without-http-cache \
  --with-poll_module \
  --with-select_module
make -j4
file objs/nginx
cp objs/nginx /out/nginx
strip /out/nginx || true
ls -la /out/nginx
