//! NetFlow v9 and IPFIX (v10) decoding: just the fields KAgent needs (addresses,
//! ports, protocol, bytes, packets). Templates are cached per exporter + domain.
//! MikroTik Traffic Flow exports either; both share the template/data-set model.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Debug, Clone, PartialEq)]
pub struct Flow {
    pub src: IpAddr,
    pub dst: IpAddr,
    pub sport: u16,
    pub dport: u16,
    pub proto: u8,
    pub bytes: u64,
    pub packets: u64,
}

#[derive(Debug, Clone)]
struct Field {
    id: u16,
    len: u16, // 65535 = variable length (IPFIX)
}

/// Template cache: (exporter key, domain/source id, template id) -> fields.
#[derive(Default)]
pub struct Decoder {
    templates: HashMap<(String, u32, u16), Vec<Field>>,
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0u64, |acc, x| (acc << 8) | *x as u64)
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode one UDP datagram. Unknown templates' data sets are skipped (they arrive
    /// with a later template refresh).
    pub fn decode(&mut self, exporter: &str, pkt: &[u8]) -> Vec<Flow> {
        match u16_at(pkt, 0) {
            Some(9) => self.decode_sets(exporter, pkt, 20, u32_at(pkt, 16).unwrap_or(0), 0, 1),
            Some(10) => {
                let len = (u16_at(pkt, 2).unwrap_or(0) as usize).min(pkt.len());
                self.decode_sets(exporter, &pkt[..len], 16, u32_at(pkt, 12).unwrap_or(0), 2, 3)
            }
            _ => Vec::new(),
        }
    }

    fn decode_sets(&mut self, exporter: &str, pkt: &[u8], mut off: usize, domain: u32, tpl_id: u16, opt_id: u16) -> Vec<Flow> {
        let mut flows = Vec::new();
        let ipfix = tpl_id == 2;
        while off + 4 <= pkt.len() {
            let (Some(set_id), Some(set_len)) = (u16_at(pkt, off), u16_at(pkt, off + 2)) else { break };
            let set_len = set_len as usize;
            if set_len < 4 || off + set_len > pkt.len() {
                break;
            }
            let body = &pkt[off + 4..off + set_len];
            if set_id == tpl_id {
                self.read_templates(exporter, domain, body, ipfix);
            } else if set_id == opt_id {
                // options templates: not needed
            } else if set_id >= 256 {
                if let Some(fields) = self.templates.get(&(exporter.to_string(), domain, set_id)).cloned() {
                    read_data(body, &fields, &mut flows);
                }
            }
            off += set_len;
        }
        flows
    }

    fn read_templates(&mut self, exporter: &str, domain: u32, mut b: &[u8], ipfix: bool) {
        while b.len() >= 4 {
            let (Some(id), Some(count)) = (u16_at(b, 0), u16_at(b, 2)) else { return };
            if id < 256 {
                return; // padding
            }
            let mut i = 4;
            let mut fields = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let (Some(fid), Some(flen)) = (u16_at(b, i), u16_at(b, i + 2)) else { return };
                i += 4;
                let mut fid = fid;
                if ipfix && fid & 0x8000 != 0 {
                    fid &= 0x7fff;
                    i += 4; // enterprise number; enterprise fields are never ours
                    fid |= 0x8000;
                }
                fields.push(Field { id: fid, len: flen });
            }
            self.templates.insert((exporter.to_string(), domain, id), fields);
            b = &b[i.min(b.len())..];
        }
    }
}

fn read_data(mut b: &[u8], fields: &[Field], out: &mut Vec<Flow>) {
    let fixed: Option<usize> = fields.iter().try_fold(0usize, |acc, f| (f.len != 65535).then_some(acc + f.len as usize));
    loop {
        if let Some(n) = fixed {
            if n == 0 || b.len() < n {
                return;
            }
        }
        let mut i = 0;
        let mut f = Flow { src: IpAddr::V4(Ipv4Addr::UNSPECIFIED), dst: IpAddr::V4(Ipv4Addr::UNSPECIFIED), sport: 0, dport: 0, proto: 0, bytes: 0, packets: 0 };
        for field in fields {
            let mut len = field.len as usize;
            if field.len == 65535 {
                let Some(&l) = b.get(i) else { return };
                i += 1;
                len = l as usize;
                if l == 255 {
                    let Some(l2) = u16_at(b, i) else { return };
                    i += 2;
                    len = l2 as usize;
                }
            }
            let Some(v) = b.get(i..i + len) else { return };
            i += len;
            match (field.id, len) {
                (1, _) | (85, _) => f.bytes = be(v),
                (2, _) | (86, _) => f.packets = be(v),
                (4, _) => f.proto = be(v) as u8,
                (7, _) => f.sport = be(v) as u16,
                (11, _) => f.dport = be(v) as u16,
                (8, 4) => f.src = IpAddr::V4(Ipv4Addr::new(v[0], v[1], v[2], v[3])),
                (12, 4) => f.dst = IpAddr::V4(Ipv4Addr::new(v[0], v[1], v[2], v[3])),
                (27, 16) => f.src = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(v).unwrap())),
                (28, 16) => f.dst = IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(v).unwrap())),
                _ => {}
            }
        }
        if i == 0 {
            return;
        }
        if !f.src.is_unspecified() || !f.dst.is_unspecified() {
            out.push(f);
        }
        b = &b[i..];
        // Remaining bytes shorter than a record are set padding.
        if fixed.is_none() && b.len() < 4 {
            return;
        }
    }
}

pub fn is_lan(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local(),
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

// ---------------------------------------------------------------- test packet builders

#[cfg(test)]
pub mod testpkt {
    /// IPFIX packet: one template (id 256: src4, dst4, sport, dport, proto, bytes8,
    /// packets8) and `flows` data records.
    pub fn ipfix(flows: &[([u8; 4], [u8; 4], u16, u16, u8, u64, u64)]) -> Vec<u8> {
        let fields: [(u16, u16); 7] = [(8, 4), (12, 4), (7, 2), (11, 2), (4, 1), (1, 8), (2, 8)];
        let mut tpl = vec![];
        tpl.extend(256u16.to_be_bytes());
        tpl.extend((fields.len() as u16).to_be_bytes());
        for (id, len) in fields {
            tpl.extend(id.to_be_bytes());
            tpl.extend(len.to_be_bytes());
        }
        let mut data = vec![];
        for (s, d, sp, dp, p, by, pk) in flows {
            data.extend(s);
            data.extend(d);
            data.extend(sp.to_be_bytes());
            data.extend(dp.to_be_bytes());
            data.push(*p);
            data.extend(by.to_be_bytes());
            data.extend(pk.to_be_bytes());
        }
        let mut body = vec![];
        body.extend(2u16.to_be_bytes());
        body.extend(((tpl.len() + 4) as u16).to_be_bytes());
        body.extend(tpl);
        body.extend(256u16.to_be_bytes());
        body.extend(((data.len() + 4) as u16).to_be_bytes());
        body.extend(data);
        let mut pkt = vec![];
        pkt.extend(10u16.to_be_bytes());
        pkt.extend(((body.len() + 16) as u16).to_be_bytes());
        pkt.extend([0u8; 8]); // export time + sequence
        pkt.extend(7u32.to_be_bytes()); // observation domain
        pkt.extend(body);
        pkt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipfix_template_then_data() {
        let pkt = testpkt::ipfix(&[([192, 168, 88, 10], [1, 1, 1, 1], 50000, 443, 6, 1500, 3), ([10, 0, 0, 2], [8, 8, 8, 8], 5353, 53, 17, 90, 1)]);
        let mut d = Decoder::new();
        let f = d.decode("r", &pkt);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0], Flow { src: "192.168.88.10".parse().unwrap(), dst: "1.1.1.1".parse().unwrap(), sport: 50000, dport: 443, proto: 6, bytes: 1500, packets: 3 });
        assert_eq!(f[1].proto, 17);
        assert!(is_lan(&f[0].src) && !is_lan(&f[0].dst));
    }

    #[test]
    fn netflow_v9_reduced_size_counters() {
        // v9: header 20 B, template set (id 0) with 4-byte counters, data set 256.
        let mut tpl = vec![];
        for (id, len) in [(8u16, 4u16), (12, 4), (7, 2), (11, 2), (4, 1), (1, 4), (2, 4)] {
            tpl.extend(id.to_be_bytes());
            tpl.extend(len.to_be_bytes());
        }
        let mut body = vec![];
        body.extend(0u16.to_be_bytes());
        body.extend(((tpl.len() + 8) as u16).to_be_bytes());
        body.extend(256u16.to_be_bytes());
        body.extend(7u16.to_be_bytes());
        body.extend(tpl);
        let mut rec = vec![192, 168, 1, 5, 93, 184, 216, 34];
        rec.extend(40000u16.to_be_bytes());
        rec.extend(80u16.to_be_bytes());
        rec.push(6);
        rec.extend(4096u32.to_be_bytes());
        rec.extend(4u32.to_be_bytes());
        rec.extend([0u8; 3]); // padding to 4 bytes
        body.extend(256u16.to_be_bytes());
        body.extend(((rec.len() + 4) as u16).to_be_bytes());
        body.extend(rec);
        let mut pkt = vec![];
        pkt.extend(9u16.to_be_bytes());
        pkt.extend(2u16.to_be_bytes());
        pkt.extend([0u8; 12]);
        pkt.extend(1u32.to_be_bytes());
        pkt.extend(body);
        let f = Decoder::new().decode("r", &pkt);
        assert_eq!(f.len(), 1, "padding must not produce a phantom record");
        assert_eq!((f[0].dport, f[0].bytes, f[0].packets), (80, 4096, 4));
    }

    #[test]
    fn data_before_template_is_skipped_and_garbage_is_safe() {
        let full = testpkt::ipfix(&[([192, 168, 1, 2], [1, 2, 3, 4], 1, 2, 6, 10, 1)]);
        let mut d = Decoder::new();
        assert!(d.decode("r", &[0, 10, 0, 3]).is_empty());
        assert!(d.decode("r", &[0xff; 64]).is_empty());
        assert_eq!(d.decode("r", &full).len(), 1);
        // A different exporter does not share templates.
        let data_only: Vec<u8> = {
            let mut p = full.clone();
            // keep header, drop the template set (bytes 16 .. 16+4+4+7*4)
            p.drain(16..16 + 4 + 4 + 28);
            let len = p.len() as u16;
            p[2..4].copy_from_slice(&len.to_be_bytes());
            p
        };
        assert!(d.decode("other", &data_only).is_empty());
        assert_eq!(d.decode("r", &data_only).len(), 1);
    }
}
