use super::*;
use crate::ech_dns_policy::CLOUDFLARE_ECH_QUERY_NAME;
use hickory_resolver::proto::{
    op::{Message, MessageType},
    rr::{
        rdata::{
            svcb::{EchConfig, SvcParamKey, SvcParamValue, SVCB},
            A, AAAA,
        },
        Name, RData, Record, RecordType,
    },
};
use http_body_util::Full;
use hyper::{body::Incoming, service::service_fn, Request, Response};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use tokio::sync::Mutex;

#[test]
fn only_forum_domains_use_public_ech_entry() {
    for host in [
        "linux.do",
        "www.linux.do",
        "a.b.linux.do",
        "LINUX.DO.",
        "ldstatic.com",
        "cdn.ldstatic.com",
        "CDN.LDSTATIC.COM.",
        "idcflare.com",
        "cdn.idcflare.com",
        "CDN.IDCFLARE.COM.",
    ] {
        assert_eq!(
            EchDnsPolicy::default().query_name(host),
            CLOUDFLARE_ECH_QUERY_NAME,
            "{host}"
        );
    }
    for host in [
        "",
        "notlinux.do",
        "linux.do.example.com",
        "evil-ldstatic.com",
        "notidcflare.com",
        "idcflare.com.example.net",
        "challenges.cloudflare.com",
        "cloudflare-ech.com",
        "example.com.",
        "127.0.0.1",
        "::1",
        "中文.example",
    ] {
        assert_eq!(EchDnsPolicy::default().query_name(host), host, "{host}");
    }
}

#[tokio::test]
async fn shared_resolvers_isolate_caches_between_policies() {
    let enabled = EchDnsPolicy::default();
    let disabled = EchDnsPolicy {
        enabled: false,
        ..Default::default()
    };
    let a = DnsResolver::shared_with_policy(
        "https://127.0.0.1/dns-query",
        None,
        false,
        None,
        enabled.clone(),
    )
    .await
    .unwrap();
    a.edge_pin.report(
        "linux.do",
        "104.21.0.10".parse().unwrap(),
        crate::edge_pin::Outcome::Connected {
            rtt: None,
            took_over: false,
        },
    );
    let b = DnsResolver::shared_with_policy(
        "https://127.0.0.1/dns-query",
        None,
        false,
        None,
        disabled,
    )
    .await
    .unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
    assert!(b.edge_pin.sticky("linux.do").is_none());
    let c = DnsResolver::shared_with_policy(
        "https://127.0.0.1/dns-query",
        None,
        false,
        None,
        enabled,
    )
    .await
    .unwrap();
    assert!(Arc::ptr_eq(&a, &c));
}

// 模拟 DoH 上游，验证实际发出的三个查询及缓存，不依赖公网 DNS。
#[tokio::test]
async fn forum_queries_share_public_entry_and_keep_original_host_cache() {
    let queries = Arc::new(Mutex::new(Vec::new()));
    let observed = queries.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint: Uri = format!("http://{}/dns-query", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let server = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let observed = observed.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: Request<Incoming>| {
                    let observed = observed.clone();
                    async move {
                        let encoded = req.uri().query().unwrap().strip_prefix("dns=").unwrap();
                        let query =
                            Message::from_vec(&URL_SAFE_NO_PAD.decode(encoded).unwrap())
                                .unwrap();
                        let question = query.queries()[0].clone();
                        let name = question.name().to_ascii();
                        let record_type = question.query_type();
                        observed.lock().await.push((name.clone(), record_type));
                        let is_public = name == "cloudflare-ech.com.";
                        let data = match record_type {
                            RecordType::A => RData::A(A(if is_public {
                                [104, 21, 0, 10]
                            } else {
                                [104, 21, 0, 11]
                            }
                            .into())),
                            RecordType::AAAA => {
                                RData::AAAA(AAAA("2606:4700::10".parse().unwrap()))
                            }
                            RecordType::HTTPS => RData::HTTPS(HTTPS(SVCB::new(
                                1,
                                Name::root(),
                                vec![(
                                    SvcParamKey::EchConfig,
                                    SvcParamValue::EchConfig(EchConfig(vec![0, 3, 1, 2, 3])),
                                )],
                            ))),
                            _ => unreachable!(),
                        };
                        let mut response = Message::new();
                        response
                            .set_id(query.id())
                            .set_message_type(MessageType::Response)
                            .add_query(question.clone())
                            .add_answer(Record::from_rdata(question.name().clone(), 120, data));
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header("content-type", "application/dns-message")
                                .body(Full::new(bytes::Bytes::from(response.to_vec().unwrap())))
                                .unwrap(),
                        )
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    let mut resolver =
        DnsResolver::with_resolver_config(ResolverConfig::cloudflare_https(), false)
            .await
            .unwrap();
    let client = Client::builder().no_proxy().build().unwrap();
    let upstream = DohUpstream::new(endpoint, client, None);
    upstream.health.lock().get_only = true;
    resolver.doh_ip = Some(upstream.clone());
    resolver.doh_ech = Some(upstream);
    resolver.force_doh_get_ip = true;
    resolver.force_doh_get_ech = true;

    let result = resolver.lookup_host("ldstatic.com", false).await.unwrap();
    assert!(result.addrs.contains(&"104.21.0.10".parse().unwrap()));
    assert_eq!(result.ech_config.unwrap().as_ref(), &[0, 3, 1, 2, 3]);
    assert!(result.ttl <= Duration::from_secs(120));
    assert!(result.ttl > Duration::from_secs(110));
    assert!(result.preferred_ip.is_none());
    let mut types = queries.lock().await.clone();
    assert_eq!(types.len(), 3);
    assert!(types.iter().all(|(name, _)| name == "cloudflare-ech.com."));
    types.sort_by_key(|(_, kind)| u16::from(*kind));
    assert_eq!(
        types.iter().map(|(_, kind)| *kind).collect::<Vec<_>>(),
        vec![RecordType::A, RecordType::AAAA, RecordType::HTTPS]
    );
    assert!(resolver.ip_cache.read().contains_key("ldstatic.com"));
    assert!(resolver.ech_cache.read().contains_key("ldstatic.com"));

    resolver.edge_pin.report(
        "ldstatic.com",
        "104.21.0.10".parse().unwrap(),
        crate::edge_pin::Outcome::Connected {
            rtt: None,
            took_over: false,
        },
    );
    assert!(resolver
        .lookup_host("ldstatic.com", false)
        .await
        .unwrap()
        .preferred_ip
        .is_some());
    assert_eq!(queries.lock().await.len(), 3);
    resolver.lookup_host("ldstatic.com", true).await.unwrap();
    assert_eq!(queries.lock().await.len(), 6);

    let unrelated = resolver.lookup_host("example.com", false).await.unwrap();
    assert!(unrelated.addrs.contains(&"104.21.0.11".parse().unwrap()));
    assert!(queries.lock().await[6..]
        .iter()
        .all(|(name, _)| name == "example.com."));
    resolver.ech_dns_policy = EchDnsPolicy {
        enabled: true,
        custom_domains: vec!["custom.example".into()],
    };
    resolver
        .lookup_host("cdn.custom.example", true)
        .await
        .unwrap();
    assert!(queries.lock().await[9..12]
        .iter()
        .all(|(name, _)| name == "cloudflare-ech.com."));
    resolver.ech_dns_policy.enabled = false;
    let normal = resolver.lookup_host("ldstatic.com", true).await.unwrap();
    assert!(normal.addrs.contains(&"104.21.0.11".parse().unwrap()));
    assert!(queries.lock().await[12..15]
        .iter()
        .all(|(name, _)| name == "ldstatic.com."));
    server.abort();
}
