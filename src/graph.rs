use anyhow::Result;
use raug::{graph::Graph, prelude::*};
use raug_ext::prelude::{BlSawOscillator, SineOscillator};
use raug_graph::{builder::IntoIndex, graph::NodeIndex};
use rosc::{OscMessage, OscPacket, OscType};
use thiserror::Error;
use tokio::net::{ToSocketAddrs, UdpSocket};

use crate::server::Server;

#[derive(Error, Debug)]
#[error("Invalid name or index")]
pub struct InvalidNameOrIndexError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameOrIndex {
    Name(String),
    Index(u32),
}

impl TryFrom<OscType> for NameOrIndex {
    type Error = InvalidNameOrIndexError;

    fn try_from(value: OscType) -> Result<Self, Self::Error> {
        match value {
            OscType::String(s) => Ok(NameOrIndex::Name(s)),
            OscType::Int(i) => Ok(NameOrIndex::Index(i as u32)),
            _ => Err(InvalidNameOrIndexError),
        }
    }
}

#[allow(clippy::from_over_into)]
impl Into<OscType> for NameOrIndex {
    fn into(self) -> OscType {
        match self {
            NameOrIndex::Name(n) => OscType::String(n),
            NameOrIndex::Index(i) => OscType::Int(i as i32),
        }
    }
}

impl IntoIndex for &NameOrIndex {
    fn into_input_idx<G: raug_graph::prelude::AbstractGraph>(
        self,
        node: &raug_graph::prelude::NodeBuilder<G>,
    ) -> Option<u32> {
        match self {
            NameOrIndex::Name(name) => node.input(name.as_str()).map(|i| i.index()).ok(),
            NameOrIndex::Index(index) => Some(*index),
        }
    }

    fn into_output_idx<G: raug_graph::prelude::AbstractGraph>(
        self,
        node: &raug_graph::prelude::NodeBuilder<G>,
    ) -> Option<u32> {
        match self {
            NameOrIndex::Name(name) => node.output(name.as_str()).map(|i| i.index()).ok(),
            NameOrIndex::Index(index) => Some(*index),
        }
    }
}

#[derive(Error, Debug)]
#[error("Invalid response: {0}")]
pub struct InvalidGraphOpResponseError(String);

#[derive(Debug, Clone, PartialEq)]
pub enum GraphOpResponse {
    NodeIndex(NodeIndex),
    None,
}

impl GraphOpResponse {
    pub fn as_node_index(&self) -> Option<&NodeIndex> {
        match self {
            GraphOpResponse::NodeIndex(idx) => Some(idx),
            _ => None,
        }
    }

    pub fn to_osc(self) -> OscPacket {
        match self {
            GraphOpResponse::NodeIndex(i) => OscPacket::Message(OscMessage {
                addr: "/response/node_index".to_string(),
                args: vec![OscType::Int(i.index() as i32)],
            }),
            GraphOpResponse::None => OscPacket::Message(OscMessage {
                addr: "/response/none".to_string(),
                args: vec![],
            }),
        }
    }

    pub fn try_from_osc(packet: &OscPacket) -> Result<Vec<GraphOpResponse>> {
        match packet {
            OscPacket::Message(msg) => match msg.addr.as_str() {
                "/response/node_index" => {
                    let [index] = &msg.args[..] else {
                        unreachable!()
                    };
                    let index = index.clone().int().unwrap() as usize;
                    Ok(vec![GraphOpResponse::NodeIndex(NodeIndex::new(index))])
                }
                "/response/none" => Ok(vec![GraphOpResponse::None]),
                msg => Err(InvalidGraphOpResponseError(msg.to_string()).into()),
            },
            OscPacket::Bundle(bund) => {
                let mut ops = vec![];
                for packet in bund.content.iter() {
                    ops.extend(GraphOpResponse::try_from_osc(packet)?);
                }
                Ok(ops)
            }
        }
    }
}

#[derive(Error, Debug)]
#[error("Invalid graph op: {0}")]
pub struct InvalidGraphOpError(String);

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum GraphOp {
    Play,
    Stop,
    AddConstantF32(f32),
    AddConstantBool(bool),
    AddConstantString(String),
    AddToMix {
        mixer_channel: usize,
        source: NodeIndex,
        source_output: NameOrIndex,
    },
    AddProcessor {
        name: String,
    },
    Connect {
        source: NodeIndex,
        source_output: NameOrIndex,
        target: NodeIndex,
        target_input: NameOrIndex,
    },
    ReplaceNode {
        replaced: NodeIndex,
        replacement: NodeIndex,
    },
}

impl GraphOp {
    pub async fn request(
        self,
        sock: &UdpSocket,
        addr: impl ToSocketAddrs,
    ) -> Result<GraphOpResponse> {
        let buf = rosc::encoder::encode(&self.to_osc())?;
        sock.send_to(&buf, addr).await?;
        let mut buf = [0u8; rosc::decoder::MTU];
        let (size, _addr) = sock.recv_from(&mut buf).await?;
        let (_, packet) = rosc::decoder::decode_udp(&buf[..size])?;
        Ok(GraphOpResponse::try_from_osc(&packet)?.remove(0))
    }

    pub fn apply(&self, server: &mut Server) -> Result<GraphOpResponse> {
        let graph = server.graph().clone();
        match self {
            GraphOp::Play => {
                server.start_graph()?;
                Ok(GraphOpResponse::None)
            }
            GraphOp::Stop => {
                server.stop_graph()?;
                Ok(GraphOpResponse::None)
            }
            GraphOp::AddToMix {
                mixer_channel,
                source,
                source_output,
            } => {
                let channel = server.mixer_channel(*mixer_channel).node();
                let NameOrIndex::Index(source_output) = source_output else {
                    todo!()
                };
                graph.with_inner(|graph| {
                    graph.connect(*source, *source_output, channel.id(), 0);
                });
                Ok(GraphOpResponse::None)
            }
            GraphOp::AddConstantF32(c) => {
                let node = graph.node(Constant::new(*c));
                Ok(GraphOpResponse::NodeIndex(node.id()))
            }
            GraphOp::AddConstantBool(c) => {
                let node = graph.node(Constant::new(*c));
                Ok(GraphOpResponse::NodeIndex(node.id()))
            }
            GraphOp::AddConstantString(c) => {
                let node = graph.node(Constant::new(Str::from(c.as_str())));
                Ok(GraphOpResponse::NodeIndex(node.id()))
            }
            GraphOp::AddProcessor { name } => {
                let node = add_proc_by_name(&graph, name)?;
                Ok(GraphOpResponse::NodeIndex(node))
            }
            GraphOp::Connect {
                source,
                source_output,
                target,
                target_input,
            } => {
                graph.connect(*source, source_output, *target, target_input)?;
                Ok(GraphOpResponse::None)
            }
            GraphOp::ReplaceNode {
                replaced,
                replacement,
            } => {
                let node = graph
                    .with_inner(|graph| graph.replace_node_gracefully(*replaced, *replacement));
                Ok(GraphOpResponse::NodeIndex(node))
            }
        }
    }

    pub fn try_from_osc(packet: &OscPacket) -> Result<Vec<GraphOp>> {
        match packet {
            OscPacket::Message(msg) => match msg.addr.as_str() {
                "/play" => Ok(vec![GraphOp::Play]),
                "/stop" => Ok(vec![GraphOp::Stop]),
                "/add_to_mix" => {
                    let [channel, index, output] = &msg.args[..] else {
                        unreachable!()
                    };
                    let channel = channel.clone().int().unwrap();
                    let index = index.clone().int().unwrap();
                    let output = output.clone().int().unwrap();
                    Ok(vec![GraphOp::AddToMix {
                        mixer_channel: channel as usize,
                        source: NodeIndex::new(index as usize),
                        source_output: NameOrIndex::Index(output as u32),
                    }])
                }
                "/add_constant_f32" => {
                    let [c] = &msg.args[..] else { unreachable!() };
                    let c = c.clone().float().unwrap();
                    Ok(vec![GraphOp::AddConstantF32(c)])
                }
                "/add_constant_bool" => {
                    let [c] = &msg.args[..] else { unreachable!() };
                    let c = c.clone().bool().unwrap();
                    Ok(vec![GraphOp::AddConstantBool(c)])
                }
                "/add_constant_string" => {
                    let [c] = &msg.args[..] else { unreachable!() };
                    let c = c.clone().string().unwrap();
                    Ok(vec![GraphOp::AddConstantString(c)])
                }
                "/add_processor" => {
                    let [name] = &msg.args[..] else {
                        unreachable!()
                    };
                    let name = name.clone().string().unwrap();
                    Ok(vec![GraphOp::AddProcessor { name }])
                }
                "/connect" => {
                    let [source, source_output, target, target_input] = &msg.args[..] else {
                        unreachable!()
                    };

                    let source = NodeIndex::new(source.clone().int().unwrap() as usize);
                    let source_output = NameOrIndex::try_from(source_output.clone())?;
                    let target = NodeIndex::new(target.clone().int().unwrap() as usize);
                    let target_input = NameOrIndex::try_from(target_input.clone())?;

                    Ok(vec![GraphOp::Connect {
                        source,
                        source_output,
                        target,
                        target_input,
                    }])
                }
                "/replace_node" => {
                    let [replaced, replacement] = &msg.args[..] else {
                        unreachable!()
                    };

                    let replaced = NodeIndex::new(replaced.clone().int().unwrap() as usize);
                    let replacement = NodeIndex::new(replacement.clone().int().unwrap() as usize);

                    Ok(vec![GraphOp::ReplaceNode {
                        replaced,
                        replacement,
                    }])
                }
                e => Err(InvalidGraphOpError(e.to_string()).into()),
            },
            OscPacket::Bundle(bund) => {
                let mut ops = vec![];
                for packet in bund.content.iter() {
                    ops.extend(GraphOp::try_from_osc(packet)?);
                }
                Ok(ops)
            }
        }
    }

    pub fn to_osc(self) -> OscPacket {
        match self {
            GraphOp::Play => OscPacket::Message(OscMessage {
                addr: "/play".to_string(),
                args: vec![],
            }),
            GraphOp::Stop => OscPacket::Message(OscMessage {
                addr: "/stop".to_string(),
                args: vec![],
            }),
            GraphOp::AddToMix {
                mixer_channel,
                source,
                source_output,
            } => OscPacket::Message(OscMessage {
                addr: "/add_to_mix".to_string(),
                args: vec![
                    OscType::Int(mixer_channel as i32),
                    OscType::Int(source.index() as i32),
                    source_output.into(),
                ],
            }),
            GraphOp::AddConstantF32(c) => OscPacket::Message(OscMessage {
                addr: "/add_constant_f32".to_string(),
                args: vec![OscType::Float(c)],
            }),
            GraphOp::AddConstantBool(c) => OscPacket::Message(OscMessage {
                addr: "/add_constant_bool".to_string(),
                args: vec![OscType::Bool(c)],
            }),
            GraphOp::AddConstantString(c) => OscPacket::Message(OscMessage {
                addr: "/add_constant_string".to_string(),
                args: vec![OscType::String(c)],
            }),
            GraphOp::AddProcessor { name } => OscPacket::Message(OscMessage {
                addr: "/add_processor".to_string(),
                args: vec![OscType::String(name)],
            }),
            GraphOp::Connect {
                source,
                source_output,
                target,
                target_input,
            } => {
                let source = OscType::Int(source.index() as i32);
                let source_output = source_output.into();
                let target = OscType::Int(target.index() as i32);
                let target_input = target_input.into();
                OscPacket::Message(OscMessage {
                    addr: "/connect".to_string(),
                    args: vec![source, source_output, target, target_input],
                })
            }
            GraphOp::ReplaceNode {
                replaced: target,
                replacement,
            } => {
                let target = OscType::Int(target.index() as i32);
                let replacement = OscType::Int(replacement.index() as i32);
                OscPacket::Message(OscMessage {
                    addr: "/replace_node".to_string(),
                    args: vec![target, replacement],
                })
            }
        }
    }
}

#[derive(Error, Debug)]
#[error("Unknown processor")]
pub struct UnknownProcessorError;

fn add_proc_by_name(graph: &Graph, name: &str) -> Result<NodeIndex> {
    macro_rules! procs {
        ($($proc:ident),* $(,)?) => {
            match name {
                $(
                    stringify!($proc) => graph.node($proc::default()),
                )*
                _ => return Err(UnknownProcessorError.into()),
            }
        };
    }
    let node = procs!(Add, Sub, Mul, Div, Neg, SineOscillator, BlSawOscillator);
    Ok(node.id())
}
