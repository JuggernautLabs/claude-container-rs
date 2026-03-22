//! Volume state types

use super::VolumeName;

/// State of a session's volumes
#[derive(Debug)]
pub struct VolumeState {
    pub session: VolumeCheck,
    pub state: VolumeCheck,
    pub cargo: VolumeCheck,
    pub npm: VolumeCheck,
    pub pip: VolumeCheck,
}

#[derive(Debug)]
pub enum VolumeCheck {
    Exists(VolumeName),
    Missing(VolumeName),
}

impl VolumeState {
    pub fn all_exist(&self) -> bool {
        matches!(&self.session, VolumeCheck::Exists(_))
            && matches!(&self.state, VolumeCheck::Exists(_))
            && matches!(&self.cargo, VolumeCheck::Exists(_))
            && matches!(&self.npm, VolumeCheck::Exists(_))
            && matches!(&self.pip, VolumeCheck::Exists(_))
    }

    pub fn missing(&self) -> Vec<&VolumeName> {
        let mut m = vec![];
        if let VolumeCheck::Missing(v) = &self.session { m.push(v); }
        if let VolumeCheck::Missing(v) = &self.state { m.push(v); }
        if let VolumeCheck::Missing(v) = &self.cargo { m.push(v); }
        if let VolumeCheck::Missing(v) = &self.npm { m.push(v); }
        if let VolumeCheck::Missing(v) = &self.pip { m.push(v); }
        m
    }
}
